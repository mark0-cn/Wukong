//! Desktop projection adapter. Query cursors and SQL live in application queries.
pub(crate) use crate::queries::{AgentSummary, Change, RunSummary};
use crate::{domain::*, queries::WorkspaceQueries, store::Store};
use anyhow::Result;
pub(crate) fn apply<T>(values: &mut Vec<T>, changes: Vec<Change<T>>, id: impl Fn(&T) -> &str) {
    for change in changes {
        if let Some(index) = values.iter().position(|value| id(value) == change.id) {
            if let Some(value) = change.value {
                values[index] = value;
            } else {
                values.remove(index);
            }
        } else if let Some(value) = change.value {
            values.push(value);
        }
    }
}

pub(crate) struct Delta {
    pub selected: Option<String>,
    pub reset: bool,
    pub runs: Vec<Change<RunSummary>>,
    pub agents: Vec<Change<AgentSummary>>,
    pub notes: Vec<Change<SharedDocument>>,
    pub findings: Vec<Change<Finding>>,
    pub events: Option<Vec<Event>>,
    pub calls: Option<Vec<ToolCallRecord>>,
    pub conversation: Option<Option<crate::gui_chat::Document>>,
}

#[derive(Default)]
pub(crate) struct Reader {
    queries: WorkspaceQueries,
    conversation: Option<crate::gui_chat::Document>,
}
impl Reader {
    pub fn read(
        &mut self,
        store: &Store,
        selected: Option<&str>,
        agent: Option<&str>,
        event_limit: usize,
    ) -> Result<Delta> {
        let mut data = self.queries.read(store, selected, agent, event_limit)?;
        let conversation = data.conversation.map(|record| {
            record.map(|record| {
                crate::gui_chat::Document::prepare(&record, self.conversation.as_ref())
            })
        });
        for record in data.narration_snapshots {
            if let Some(summary) = data
                .agents
                .iter_mut()
                .find(|change| change.id == record.id)
                .and_then(|change| change.value.as_mut())
            {
                summary.public_replies = crate::gui_activity::public_replies(&record)
                    .into_iter()
                    .rev()
                    .take(3)
                    .collect();
                summary.public_replies.reverse();
            }
        }
        if let Some(conversation) = &conversation {
            self.conversation = conversation.clone();
        }
        Ok(Delta {
            selected: data.selected,
            reset: data.reset,
            runs: data.runs,
            agents: data.agents,
            notes: data.notes,
            findings: data.findings,
            events: data.events,
            calls: data.calls,
            conversation,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinator::Coordinator;
    use serde_json::json;

    #[test]
    #[ignore = "manual performance baseline; run with --ignored --nocapture"]
    fn workspace_history_performance_baseline() -> Result<()> {
        for count in [1_000, 10_000, 100_000] {
            let directory = tempfile::tempdir()?;
            let target = directory.path().join("sample");
            std::fs::write(&target, b"fixture")?;
            let store = Store::open(directory.path().join("state"))?;
            let run = Coordinator::new(store.clone()).create_run(
                &target,
                "benchmark",
                Budget::default(),
                json!({}),
            )?;
            store.transaction(|tx| {
                for index in 0..count {
                    Store::tx_event(
                        tx,
                        &run.id,
                        Some(&run.root_id),
                        "benchmark",
                        json!({"index":index}),
                    )?;
                }
                Ok(())
            })?;
            let mut reader = Reader::default();
            let started = std::time::Instant::now();
            let initial = reader.read(&store, Some(&run.id), None, 500)?;
            let initial_ms = started.elapsed().as_secs_f64() * 1000.0;
            assert_eq!(initial.events.as_ref().unwrap().len(), 500);
            let started = std::time::Instant::now();
            for _ in 0..100 {
                let unchanged = reader.read(&store, Some(&run.id), None, 500)?;
                assert!(
                    unchanged.events.is_none()
                        && unchanged.calls.is_none()
                        && unchanged.agents.is_empty()
                );
            }
            let idle_ms = started.elapsed().as_secs_f64() * 10.0;
            println!(
                "history={count} initial_ms={initial_ms:.3} idle_mean_ms={idle_ms:.3} loaded_events=500"
            );
        }
        Ok(())
    }

    #[test]
    fn background_queries_deliver_only_changes_and_bounded_history() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let store = Store::open(directory.path())?;
        let run = Coordinator::new(store.clone()).create_run(
            &std::env::current_exe()?,
            "fixture",
            Budget::default(),
            json!({"large_config":"x".repeat(10_000)}),
        )?;
        for index in 0..20 {
            store.event(
                &run.id,
                Some(&run.root_id),
                "fixture",
                json!({"index":index}),
            )?;
        }
        let mut reader = Reader::default();
        let initial = reader.read(&store, Some(&run.id), None, 5)?;
        assert_eq!(initial.runs.len(), 1);
        assert_eq!(initial.agents.len(), 1);
        assert_eq!(initial.events.as_ref().unwrap().len(), 5);
        assert!(initial.conversation.unwrap().is_none());
        let unchanged = reader.read(&store, Some(&run.id), None, 5)?;
        assert!(unchanged.runs.is_empty() && unchanged.agents.is_empty());
        assert!(unchanged.events.is_none() && unchanged.calls.is_none());
        let external = Store::open(directory.path())?;
        let mut agent: AgentSnapshot = external.require("agent", &run.root_id)?;
        agent.name = "changed externally".into();
        external.put("agent", &agent.id, &run.id, &agent)?;
        let changed = reader.read(&store, Some(&run.id), None, 5)?;
        assert_eq!(changed.agents.len(), 1);
        assert_eq!(
            changed.agents[0].value.as_ref().unwrap().name,
            "changed externally"
        );
        let older = reader.read(&store, Some(&run.id), None, 10)?;
        assert_eq!(older.events.unwrap().len(), 10);
        external.transaction(|tx| {
            tx.execute(
                "DELETE FROM entities WHERE kind='agent' AND id=?1",
                [&agent.id],
            )?;
            Ok(())
        })?;
        let deleted = reader.read(&store, Some(&run.id), None, 10)?;
        assert!(deleted.agents[0].value.is_none());
        Ok(())
    }

    #[test]
    fn metadata_changes_do_not_reload_a_conversation_but_checkpoint_changes_do() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let store = Store::open(directory.path())?;
        let target = directory.path().join("sample");
        std::fs::write(&target, b"fixture")?;
        let run = Coordinator::new(store.clone()).create_run(
            &target,
            "fixture",
            Budget::default(),
            json!({}),
        )?;
        let checkpoint =
            json!({"messages":[{"role":"assistant","content":[{"type":"text","text":"first"}]}]});
        store.checkpoint_and_ack(&run.root_id, checkpoint.clone(), &[])?;
        let mut reader = Reader::default();
        assert!(
            reader
                .read(&store, Some(&run.id), Some(&run.root_id), 5)?
                .conversation
                .flatten()
                .is_some()
        );
        store.transaction(|tx| {
            let mut agent: AgentMetadata =
                Store::tx_get_metadata(tx, "agent", &run.root_id)?.unwrap();
            agent.total_tokens += 1;
            Store::tx_put_metadata(tx, "agent", &agent.id, &run.id, &agent)
        })?;
        assert!(
            reader
                .read(&store, Some(&run.id), Some(&run.root_id), 5)?
                .conversation
                .is_none()
        );
        let mut changed = checkpoint;
        changed["messages"]
            .as_array_mut()
            .unwrap()
            .push(json!({"role":"assistant","content":[{"type":"text","text":"second"}]}));
        store.checkpoint_and_ack(&run.root_id, changed, &[])?;
        assert!(
            reader
                .read(&store, Some(&run.id), Some(&run.root_id), 5)?
                .conversation
                .flatten()
                .is_some()
        );
        Ok(())
    }
}
