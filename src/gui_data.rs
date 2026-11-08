//! Background desktop queries. The GUI receives bounded history and metadata
//! changes; it never needs to scan tool history or deserialize every checkpoint.
use crate::{domain::*, store::Store};
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, de::DeserializeOwned};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct RunSummary {
    pub id: String,
    pub target_path: PathBuf,
    pub target_sha256: String,
    pub task: String,
    pub status: Status,
    pub root_id: String,
    pub budget: Budget,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub total_tokens: u64,
    pub cost_usd: f64,
    pub summary: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct AgentSummary {
    pub id: String,
    pub name: String,
    pub role: Role,
    pub generation: u64,
    pub task: String,
    pub status: Status,
    pub depth: u32,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub turns: usize,
    pub total_tokens: u64,
    pub summary: Option<String>,
    pub error: Option<String>,
    #[serde(default)]
    pub public_replies: Vec<String>,
}

pub(crate) struct Change<T> {
    id: String,
    pub value: Option<T>,
}

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
    root: PathBuf,
    revision: i64,
    selected: Option<String>,
    agent: Option<String>,
    conversation: Option<crate::gui_chat::Document>,
    events: Vec<Event>,
    event_limit: usize,
    conversation_revision: Option<i64>,
    calls: BTreeMap<String, ToolCallRecord>,
}

fn changes<T: DeserializeOwned>(
    tx: &Connection,
    kind: &str,
    run: Option<&str>,
    after: i64,
    expression: &str,
) -> Result<Vec<Change<T>>> {
    let sql = format!(
        "SELECT r.id,{expression} FROM entity_revisions r
        LEFT JOIN entities e ON e.kind=r.kind AND e.id=r.id
        WHERE r.kind=?1 AND (?2 IS NULL OR r.run_id=?2) AND r.revision>?3 ORDER BY r.revision"
    );
    let mut query = tx.prepare(&sql)?;
    let rows = query.query_map(params![kind, run, after], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
    })?;
    rows.map(|row| {
        let (id, data) = row?;
        Ok(Change {
            id,
            value: data.map(|data| serde_json::from_str(&data)).transpose()?,
        })
    })
    .collect()
}

fn read_reply_text(tx: &Connection, run: &str, agent: &str) -> Result<Vec<String>> {
    let mut query = tx.prepare(
        "SELECT data FROM events WHERE run_id=?1 AND agent_id=?2
        AND kind='model.text' ORDER BY seq DESC LIMIT 3",
    )?;
    let rows = query.query_map(params![run, agent], |row| row.get::<_, String>(0))?;
    let mut replies = Vec::new();
    for row in rows {
        let value: serde_json::Value = serde_json::from_str(&row?)?;
        if let Some(text) = value["text"].as_str() {
            replies.push(text.to_owned());
        }
    }
    replies.reverse();
    Ok(replies)
}

fn visible_calls(
    tx: &Connection,
    run: &str,
    after: i64,
    events: &[Event],
    previous: &BTreeMap<String, ToolCallRecord>,
    reset: bool,
) -> Result<Option<Vec<ToolCallRecord>>> {
    let ids: BTreeSet<_> = events
        .iter()
        .filter_map(|event| event.data["call_id"].as_str().map(str::to_owned))
        .collect();
    let mut query = tx.prepare(
        "SELECT id FROM entity_revisions WHERE kind='call' AND run_id=?1 AND revision>?2",
    )?;
    let changed: BTreeSet<String> = query
        .query_map(params![run, after], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    if !reset
        && ids.len() == previous.len()
        && ids
            .iter()
            .all(|id| previous.contains_key(id) && !changed.contains(id))
    {
        return Ok(None);
    }
    let mut calls = Vec::with_capacity(ids.len());
    for id in ids {
        if !reset
            && !changed.contains(&id)
            && let Some(call) = previous.get(&id)
        {
            calls.push(call.clone());
        } else if let Some(call) = Store::tx_get(tx, "call", &id)? {
            calls.push(call);
        }
    }
    Ok(Some(calls))
}

impl Reader {
    pub fn read(
        &mut self,
        store: &Store,
        selected: Option<&str>,
        agent: Option<&str>,
        event_limit: usize,
    ) -> Result<Delta> {
        if self.root != store.root() {
            *self = Self {
                root: store.root().to_owned(),
                ..Self::default()
            };
        }
        // Cursor advancement and its rows share a snapshot, including external CLI writes.
        let (mut delta, revision, conversation_revision, conversation, legacy_replies) = store.read_transaction(|tx| {
            let revision: i64 = tx.query_row("SELECT COALESCE(MAX(revision),0) FROM entity_revisions", [], |row| row.get(0))?;
            let runs = changes(tx, "run", None, self.revision, "json_remove(e.data,'$.config')")?;
            let selected = match selected {
                Some(id) => Some(id.to_owned()),
                None => tx.query_row("SELECT id FROM entities WHERE kind='run' ORDER BY rowid DESC LIMIT 1", [], |row| row.get::<_, String>(0)).optional()?,
            };
            let reset = self.selected != selected || self.revision == 0;
            let after = if reset { 0 } else { self.revision };
            let mut delta = Delta { selected, reset, runs, agents: vec![], notes: vec![], findings: vec![], events: None, calls: None, conversation: None };
            let mut conversation_revision = None;
            let mut conversation = None;
            let mut legacy_replies = Vec::new();
            if let Some(run) = delta.selected.as_deref() {
                delta.agents = changes::<AgentSummary>(tx, "agent", Some(run), after, "json_remove(e.data,'$.active_skills')")?;
                delta.notes = changes(tx, "note", Some(run), after, "e.data")?;
                delta.findings = changes(tx, "finding", Some(run), after, "e.data")?;
                let latest: i64 = tx.query_row("SELECT COALESCE(MAX(seq),0) FROM events WHERE run_id=?1", [run], |row| row.get(0))?;
                let last = self.events.last().map_or(0, |event| event.seq);
                if reset || latest != last || self.event_limit != event_limit {
                    delta.events = Some(Store::tx_recent_events(tx, run, event_limit)?);
                }
                let window = delta.events.as_deref().unwrap_or(&self.events);
                // Narration can arrive without a metadata mutation.
                let narrators: BTreeSet<_> = window.iter().filter(|event| event.seq > last && event.kind == "model.text").filter_map(|event| event.agent_id.as_ref()).collect();
                for id in narrators {
                    if !delta.agents.iter().any(|change| &change.id == id)
                        && let Some(summary) = Store::tx_get_metadata(tx, "agent", id)? {
                        delta.agents.push(Change { id: id.clone(), value: Some(summary) });
                    }
                }
                for change in &mut delta.agents {
                    if let Some(summary) = &mut change.value {
                        summary.public_replies = read_reply_text(tx, run, &summary.id)?;
                        if reset && summary.public_replies.is_empty()
                            && let Some(record) = Store::tx_get::<AgentRecord>(tx, "agent", &summary.id)? {
                            legacy_replies.push(record);
                        }
                    }
                }
                delta.calls = visible_calls(tx, run, after, window, &self.calls, reset)?;
                if let Some(id) = agent {
                    conversation_revision = tx.query_row("SELECT revision FROM agent_conversations WHERE agent_id=?1 AND run_id=?2", params![id, run], |row| row.get(0)).optional()?;
                }
                let deleted = delta.agents.iter().any(|change| Some(change.id.as_str()) == agent && change.value.is_none());
                if reset || self.agent.as_deref() != agent || conversation_revision != self.conversation_revision || deleted {
                    let record = agent.map(|id| Store::tx_get::<AgentRecord>(tx, "agent", id)).transpose()?.flatten();
                    conversation = Some(record.filter(|record| record.run_id == run));
                }
            }
            Ok((delta, revision, conversation_revision, conversation, legacy_replies))
        })?;
        // Parsing and hashing large conversations must not hold a SQLite snapshot or reader lock.
        if let Some(record) = conversation {
            delta.conversation = Some(record.map(|record| {
                crate::gui_chat::Document::prepare(&record, self.conversation.as_ref())
            }));
        }
        for record in legacy_replies {
            if let Some(summary) = delta
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
        self.revision = revision;
        self.conversation_revision = conversation_revision;
        self.selected = delta.selected.clone();
        self.agent = agent.map(str::to_owned);
        self.event_limit = event_limit;
        if let Some(conversation) = &delta.conversation {
            self.conversation = conversation.clone();
        }
        if let Some(events) = &delta.events {
            self.events = events.clone();
        }
        if let Some(calls) = &delta.calls {
            self.calls = calls
                .iter()
                .map(|call| (call.id.clone(), call.clone()))
                .collect();
        }
        Ok(delta)
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
        let mut agent: AgentRecord = external.require("agent", &run.root_id)?;
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
            let mut agent: AgentRecord =
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
