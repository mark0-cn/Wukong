//! Background desktop queries. The GUI receives bounded history and metadata
//! changes; it never needs to scan tool history or deserialize every checkpoint.
use crate::{domain::*, store::Store};
use anyhow::Result;
use rusqlite::{Connection, params};
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
    value: Option<T>,
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
        // All cursors and data are observed together, including external CLI writes.
        let (delta, revision) = store.transaction(|tx| {
            let revision: i64 = tx.query_row("SELECT COALESCE(MAX(revision),0) FROM entity_revisions",[],|row|row.get(0))?;
            let runs = changes(tx,"run",None,self.revision,"json_remove(e.data,'$.config')")?;
            let selected = selected.map(str::to_owned).or_else(|| {
                tx.query_row("SELECT id FROM entities WHERE kind='run' ORDER BY rowid DESC LIMIT 1",[],|row|row.get::<_,String>(0)).ok()
            });
            let reset = self.selected != selected || self.revision == 0;
            let after = if reset {0} else {self.revision};
            let mut agents = vec![];
            let mut notes = vec![];
            let mut findings = vec![];
            let mut events = None;
            let mut calls = None;
            let mut conversation = None;
            if let Some(run) = selected.as_deref() {
                agents = changes::<AgentSummary>(tx,"agent",Some(run),after,"json_remove(e.data,'$.checkpoint','$.inherited_context','$.active_skills')")?;
                for change in &mut agents {
                    if let Some(summary) = &mut change.value {
                        let mut query = tx.prepare("SELECT data FROM events WHERE run_id=?1 AND agent_id=?2 AND kind='model.text' ORDER BY seq DESC LIMIT 3")?;
                        let rows = query.query_map(params![run,summary.id],|row|row.get::<_,String>(0))?;
                        for row in rows {
                            let data: serde_json::Value=serde_json::from_str(&row?)?;
                            if let Some(text)=data["text"].as_str() {summary.public_replies.push(text.to_owned());}
                        }
                        summary.public_replies.reverse();
                        if reset && summary.public_replies.is_empty() {
                            // Older runs predate model.text events. Read once on selection.
                            if let Some(record)=Store::tx_get::<AgentRecord>(tx,"agent",&summary.id)? {
                                summary.public_replies=crate::gui_activity::public_replies(&record).into_iter().rev().take(3).collect();
                                summary.public_replies.reverse();
                            }
                        }
                    }
                }
                notes=changes(tx,"note",Some(run),after,"e.data")?;
                findings=changes(tx,"finding",Some(run),after,"e.data")?;
                let latest: i64=tx.query_row("SELECT COALESCE(MAX(seq),0) FROM events WHERE run_id=?1",[run],|row|row.get(0))?;
                if reset || latest != self.events.last().map_or(0,|event|event.seq) || self.event_limit != event_limit {
                    events=Some(Store::tx_recent_events(tx,run,event_limit)?);
                }
                let window=events.as_ref().unwrap_or(&self.events);
                let ids: BTreeSet<_>=window.iter().filter_map(|event|event.data["call_id"].as_str().map(str::to_owned)).collect();
                let changed: BTreeSet<String> = {
                    let mut query=tx.prepare("SELECT id FROM entity_revisions WHERE kind='call' AND run_id=?1 AND revision>?2")?;
                    query.query_map(params![run,after],|row|row.get(0))?.collect::<rusqlite::Result<_>>()?
                };
                let mut visible=if reset {BTreeMap::new()} else {self.calls.clone()};
                visible.retain(|id,_|ids.contains(id));
                for id in &ids {
                    if !visible.contains_key(id) || changed.contains(id) {
                        if let Some(call)=Store::tx_get(tx,"call",id)? {visible.insert(id.clone(),call);}
                    }
                }
                if reset || events.is_some() || ids.iter().any(|id|changed.contains(id)) {calls=Some(visible.into_values().collect());}
                if reset || self.agent.as_deref()!=agent || agents.iter().any(|change|Some(change.id.as_str())==agent) {
                    let record=agent.map(|id|Store::tx_get::<AgentRecord>(tx,"agent",id)).transpose()?.flatten();
                    conversation=Some(record.filter(|record|record.run_id==run).map(|record|crate::gui_chat::Document::prepare(&record,self.conversation.as_ref())));
                }
            }
            Ok((Delta {selected,reset,runs,agents,notes,findings,events,calls,conversation},revision))
        })?;
        self.revision = revision;
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
}
