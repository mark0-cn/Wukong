//! Local business activity from the durable Store, explicitly not OTel spans.

use anyhow::{Result, ensure};
use chrono::{DateTime, Utc};
use rusqlite::params;
use serde::Serialize;
use serde_json::{Value, json};

use crate::{
    domain::{AgentMetadata, CallState, Event, JobRecord, RunRecord, Status, ToolCallRecord},
    storage::Store,
};

pub const MAX_LOCAL_EVENTS: usize = 500;
const MAX_LOCAL_AGENTS: usize = 256;
const MAX_EVENT_BYTES: usize = 16 * 1024;
const MAX_TIMELINE_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalActivityKind {
    LocalBusinessActivity,
}

#[derive(Debug, Serialize)]
pub struct LocalRun {
    pub id: String,
    pub status: Status,
    pub root_id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub total_tokens: u64,
    pub cost_usd: f64,
    pub summary: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct LocalAgent {
    pub id: String,
    pub parent_id: Option<String>,
    pub name: String,
    pub role: crate::domain::Role,
    pub status: Status,
    pub generation: u64,
    pub turns: usize,
    pub total_tokens: u64,
    pub cost_usd: f64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Default, Serialize)]
pub struct ToolCounts {
    pub total: usize,
    pub prepared: usize,
    pub running: usize,
    pub completed: usize,
    pub failed: usize,
    pub uncertain: usize,
    pub side_effecting: usize,
}

#[derive(Debug, Default, Serialize)]
pub struct ResourceCounts {
    pub jobs: usize,
    pub active_jobs: usize,
    pub terminal_jobs: usize,
    pub evidence_pending: usize,
}

#[derive(Debug, Serialize)]
pub struct LocalEvent {
    pub seq: i64,
    pub agent_id: Option<String>,
    pub kind: String,
    pub created_at: DateTime<Utc>,
    pub data: Value,
    pub data_truncated: bool,
}

#[derive(Debug, Serialize)]
pub struct LocalActivity {
    pub activity_kind: LocalActivityKind,
    pub run: LocalRun,
    pub agents: Vec<LocalAgent>,
    pub events: Vec<LocalEvent>,
    pub tools: ToolCounts,
    pub resources: ResourceCounts,
    pub next_cursor: Option<String>,
    pub has_more: bool,
    pub partial: bool,
    pub truncated: bool,
    pub stale: bool,
    pub queried_at: DateTime<Utc>,
}

pub(crate) fn query(
    store: &Store,
    run_id: &str,
    after: i64,
    limit: usize,
) -> Result<LocalActivity> {
    super::validate_run_id(run_id)?;
    ensure!(
        after >= 0,
        "local activity cursor must be a non-negative sequence"
    );
    ensure!(
        (1..=MAX_LOCAL_EVENTS).contains(&limit),
        "local activity limit must be 1–500"
    );

    store.read_transaction(|tx| {
        let run: RunRecord = Store::tx_get(tx, "run", run_id)?
            .ok_or_else(|| anyhow::anyhow!("run does not exist"))?;
        ensure!(run.id == run_id, "run ownership mismatch");
        let agents = Store::tx_list_metadata(tx, "agent", run_id)?;
        ensure!(
            agents.iter().all(|agent| agent.run_id == run_id),
            "agent does not belong to the requested run"
        );
        let calls = Store::tx_list::<ToolCallRecord>(tx, "call", run_id)?;
        ensure!(
            calls.iter().all(|call| call.run_id == run_id),
            "tool call does not belong to the requested run"
        );
        let jobs = Store::tx_list::<JobRecord>(tx, "job", run_id)?;
        ensure!(
            jobs.iter().all(|job| job.run_id == run_id),
            "job does not belong to the requested run"
        );

        let mut statement = tx.prepare(
            "SELECT seq,agent_id,kind,data,created_at FROM events
             WHERE run_id=?1 AND seq>?2 ORDER BY seq LIMIT ?3",
        )?;
        let rows = statement.query_map(params![run_id, after, limit as i64 + 1], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;
        let mut raw_events = Vec::with_capacity(limit + 1);
        for row in rows {
            let (seq, agent_id, kind, data, created_at) = row?;
            raw_events.push(Event {
                seq,
                run_id: run_id.into(),
                agent_id,
                kind,
                data: serde_json::from_str(&data)?,
                created_at: created_at.parse()?,
            });
        }
        let has_more = raw_events.len() > limit;
        raw_events.truncate(limit);
        let mut timeline_bytes = 0_usize;
        let mut event_truncated = false;
        let events = raw_events
            .into_iter()
            .map(|event| {
                let size = serde_json::to_vec(&event.data)
                    .map(|bytes| bytes.len())
                    .unwrap_or(MAX_EVENT_BYTES + 1);
                let allowed =
                    MAX_EVENT_BYTES.min(MAX_TIMELINE_BYTES.saturating_sub(timeline_bytes));
                let omit = size > allowed;
                if omit {
                    event_truncated = true;
                } else {
                    timeline_bytes = timeline_bytes.saturating_add(size);
                }
                LocalEvent {
                    seq: event.seq,
                    agent_id: event.agent_id,
                    kind: event.kind,
                    created_at: event.created_at,
                    data: if omit {
                        json!({"omitted": true, "original_bytes": size})
                    } else {
                        event.data
                    },
                    data_truncated: omit,
                }
            })
            .collect::<Vec<_>>();
        let next_cursor = has_more
            .then(|| events.last().map(|event| event.seq.to_string()))
            .flatten();

        let mut tools = ToolCounts::default();
        for call in &calls {
            tools.total += 1;
            tools.side_effecting += usize::from(call.side_effecting);
            match call.state {
                CallState::Prepared => tools.prepared += 1,
                CallState::Running => tools.running += 1,
                CallState::Completed => tools.completed += 1,
                CallState::Failed => tools.failed += 1,
                CallState::Uncertain => tools.uncertain += 1,
            }
        }
        let mut resources = ResourceCounts {
            jobs: jobs.len(),
            ..ResourceCounts::default()
        };
        for job in &jobs {
            if job.status.state.terminal() {
                resources.terminal_jobs += 1;
            } else {
                resources.active_jobs += 1;
            }
            resources.evidence_pending += usize::from(!job.evidence_ready());
        }

        let agents_truncated = agents.len() > MAX_LOCAL_AGENTS;
        let agents = agents
            .into_iter()
            .take(MAX_LOCAL_AGENTS)
            .map(local_agent)
            .collect();
        Ok(LocalActivity {
            activity_kind: LocalActivityKind::LocalBusinessActivity,
            run: LocalRun {
                id: run.id,
                status: run.status,
                root_id: run.root_id,
                created_at: run.created_at,
                updated_at: run.updated_at,
                total_tokens: run.total_tokens,
                cost_usd: run.cost_usd,
                summary: run.summary,
            },
            agents,
            events,
            tools,
            resources,
            next_cursor,
            has_more,
            partial: false,
            truncated: has_more || agents_truncated || event_truncated,
            stale: false,
            queried_at: Utc::now(),
        })
    })
}

fn local_agent(agent: AgentMetadata) -> LocalAgent {
    LocalAgent {
        id: agent.id,
        parent_id: agent.parent_id,
        name: agent.name,
        role: agent.role,
        status: agent.status,
        generation: agent.generation,
        turns: agent.turns,
        total_tokens: agent.total_tokens,
        cost_usd: agent.cost_usd,
        created_at: agent.created_at,
        updated_at: agent.updated_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Budget;
    use std::path::PathBuf;

    fn store_with_run() -> Result<(tempfile::TempDir, Store)> {
        let directory = tempfile::tempdir()?;
        let store = Store::open(directory.path().join("state"))?;
        let now = Utc::now();
        let run = RunRecord {
            id: "run-1".into(),
            target_path: PathBuf::from("fixture"),
            target_sha256: "a".repeat(64),
            bundle: None,
            task: "fixture".into(),
            status: Status::Running,
            root_id: "root".into(),
            config: json!({}),
            budget: Budget::default(),
            created_at: now,
            updated_at: now,
            total_tokens: 12,
            cost_usd: 0.25,
            summary: None,
        };
        store.put("run", &run.id, &run.id, &run)?;
        Ok((directory, store))
    }

    #[test]
    fn rejects_unknown_run_and_invalid_page() -> Result<()> {
        let (_directory, store) = store_with_run()?;
        assert!(query(&store, "missing", 0, 100).is_err());
        assert!(query(&store, "run-1", 0, 0).is_err());
        assert!(query(&store, "run-1", -1, 100).is_err());
        Ok(())
    }

    #[test]
    fn event_pages_are_bounded_and_ordered() -> Result<()> {
        let (_directory, store) = store_with_run()?;
        store.event("run-1", None, "one", json!({"value": 1}))?;
        store.event("run-1", None, "two", json!({"value": 2}))?;
        let first = query(&store, "run-1", 0, 1)?;
        assert_eq!(first.events[0].kind, "one");
        assert!(first.has_more && first.truncated);
        let second = query(
            &store,
            "run-1",
            first.next_cursor.as_deref().unwrap().parse()?,
            1,
        )?;
        assert_eq!(second.events[0].kind, "two");
        assert!(!second.has_more);
        Ok(())
    }

    #[test]
    fn oversized_event_payload_is_replaced() -> Result<()> {
        let (_directory, store) = store_with_run()?;
        store.event(
            "run-1",
            None,
            "large",
            json!({"text": "x".repeat(MAX_EVENT_BYTES + 1)}),
        )?;
        let activity = query(&store, "run-1", 0, 1)?;
        assert!(activity.events[0].data_truncated);
        assert_eq!(activity.events[0].data["omitted"], true);
        Ok(())
    }
}
