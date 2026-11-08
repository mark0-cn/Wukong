//! SQLite implementation of incremental workspace projections.
use super::*;
use crate::queries::*;
use std::collections::{BTreeMap, BTreeSet};
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

impl WorkspaceQueries {
    pub fn read(
        &mut self,
        store: &Store,
        selected: Option<&str>,
        agent: Option<&str>,
        event_limit: usize,
    ) -> Result<WorkspaceDelta> {
        if self.root != store.root() {
            *self = Self {
                root: store.root().to_owned(),
                ..Self::default()
            };
        }
        // Cursor advancement and its rows share a snapshot, including external CLI writes.
        let (mut delta, revision, conversation_revision, conversation, narration_snapshots) = store.read_transaction(|tx| {
            let revision: i64 = tx.query_row("SELECT COALESCE(MAX(revision),0) FROM entity_revisions", [], |row| row.get(0))?;
            let runs = changes(tx, "run", None, self.revision, "json_remove(e.data,'$.config')")?;
            let selected = match selected {
                Some(id) => Some(id.to_owned()),
                None => tx.query_row("SELECT id FROM entities WHERE kind='run' ORDER BY rowid DESC LIMIT 1", [], |row| row.get::<_, String>(0)).optional()?,
            };
            let reset = self.selected != selected || self.revision == 0;
            let after = if reset { 0 } else { self.revision };
            let mut delta = WorkspaceDelta { selected, reset, runs, agents: vec![], notes: vec![], findings: vec![], events: None, calls: None, conversation: None, operator_messages: None, narration_snapshots:vec![] };
            let mut conversation_revision = None;
            let mut conversation = None;
            let mut narration_snapshots = Vec::new();
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
                        delta.agents.push(Change { id: id.clone(), value: Some(summary.into()) });
                    }
                }
                for change in &mut delta.agents {
                    if let Some(summary) = &mut change.value {
                        summary.public_replies = read_reply_text(tx, run, &summary.id)?;
                        if reset && summary.public_replies.is_empty()
                            && let Some(record) = Store::tx_agent_snapshot(tx, &summary.id)? {
                            narration_snapshots.push(record);
                        }
                    }
                }
                delta.calls = visible_calls(tx, run, after, window, &self.calls, reset)?;
                if let Some(id) = agent {
                    conversation_revision = tx.query_row("SELECT revision FROM agent_conversations WHERE agent_id=?1 AND run_id=?2", params![id, run], |row| row.get(0)).optional()?;
                }
                let mut query = tx.prepare("SELECT id,content,acknowledged,created_at,envelope FROM messages WHERE run_id=?1 AND recipient=?2 AND sender='operator' ORDER BY id DESC LIMIT 500")?;
                let records = query.query_map(params![run, agent.unwrap_or("")], |row| Ok((row.get::<_, i64>(0)?,row.get::<_,String>(1)?,row.get::<_,bool>(2)?,row.get::<_,String>(3)?,row.get::<_,String>(4)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
                let digest = serde_json::to_string(&records)?;
                if reset || self.agent.as_deref() != agent || digest != self.operator_digest {
                    let mut messages = Vec::with_capacity(records.len());
                    for (id,content,acknowledged,created_at,envelope) in records.into_iter().rev() {
                        messages.push(Message { id, run_id: run.into(), sender: "operator".into(), recipient: agent.unwrap_or("").into(), content, acknowledged, created_at: created_at.parse()?, envelope: serde_json::from_str(&envelope)? });
                    }
                    delta.operator_messages = Some(messages);
                }
                let deleted = delta.agents.iter().any(|change| Some(change.id.as_str()) == agent && change.value.is_none());
                if reset || self.agent.as_deref() != agent || conversation_revision != self.conversation_revision || deleted {
                    let record = agent.map(|id| Store::tx_agent_snapshot(tx, id)).transpose()?.flatten();
                    conversation = Some(record.filter(|record| record.run_id == run));
                }
            }
            Ok((delta, revision, conversation_revision, conversation, narration_snapshots))
        })?;
        delta.conversation = conversation;
        delta.narration_snapshots = narration_snapshots;
        if let Some(messages) = &delta.operator_messages {
            let records = messages
                .iter()
                .rev()
                .map(|message| {
                    (
                        message.id,
                        message.content.clone(),
                        message.acknowledged,
                        message.created_at.to_rfc3339(),
                        serde_json::to_string(&message.envelope).unwrap_or_default(),
                    )
                })
                .collect::<Vec<_>>();
            self.operator_digest = serde_json::to_string(&records)?;
        }
        self.revision = revision;
        self.conversation_revision = conversation_revision;
        self.selected = delta.selected.clone();
        self.agent = agent.map(str::to_owned);
        self.event_limit = event_limit;
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

impl crate::queries::AssessmentQueries<'_> {
    pub fn runs(&self) -> Result<Vec<RunSummary>> {
        self.store.read_transaction(|tx| {
            let mut query=tx.prepare("SELECT json_remove(data,'$.config') FROM entities WHERE kind='run' ORDER BY rowid DESC")?;
            let rows=query.query_map([],|row|row.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            rows.into_iter().map(|row|Ok(serde_json::from_str(&row)?)).collect()
        })
    }
    pub fn status(&self, run: &str, limit: usize) -> Result<RunStatus> {
        self.store.read_transaction(|tx| {
            let data: String = tx.query_row(
                "SELECT json_remove(data,'$.config') FROM entities WHERE kind='run' AND id=?1",
                [run],
                |row| row.get(0),
            )?;
            Ok(RunStatus {
                run: serde_json::from_str(&data)?,
                agents: Store::tx_list_metadata(tx, "agent", run)?
                    .into_iter()
                    .map(AgentSummary::from)
                    .collect(),
                events: Store::tx_recent_events(tx, run, limit.min(1000))?,
            })
        })
    }
}
