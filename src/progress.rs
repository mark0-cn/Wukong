//! Persist bounded public snapshots. Rig owns streaming, retries and request lifetimes.
use crate::{domain::*, store::Store};
use anyhow::{Context, Result};
use serde_json::{Value, json};

pub(crate) const MAX_PUBLIC_CHARS: usize = 64 * 1024;

pub(crate) fn publish(
    store: &Store,
    key: &PublicReply,
    text: &str,
    state: ReplyState,
) -> Result<Vec<Value>> {
    let mut chars = text.chars();
    let text: String = chars.by_ref().take(MAX_PUBLIC_CHARS).collect();
    let truncated = chars.next().is_some();
    store.write(|tx| {
        let agent: AgentMetadata = tx.get_record(&key.agent_id)?.context("public reply agent missing")?;
        // Late fragments cannot write across a follow-up generation or accounted turn.
        if agent.run_id != key.run_id || agent.generation != key.generation || agent.turns != key.turn { return Ok(vec![]); }
        let previous: Option<PublicReply> = tx.get_record(&key.id)?;
        if previous.is_none() && text.trim().is_empty() { return Ok(vec![]); }
        let fresh = previous.is_none();
        let mut reply = previous.unwrap_or_else(|| key.clone());
        // A late stream fragment cannot reopen a final public reply.
        if reply.state != ReplyState::Streaming { return Ok(vec![]); }
        if reply.text == text && reply.state == state && reply.truncated == truncated { return Ok(vec![]); }
        if text.is_empty() && !reply.text.is_empty() {
            reply.state = ReplyState::Interrupted;
        } else {
            reply.text = text.clone(); reply.state = state; reply.truncated = truncated;
        }
        reply.revision += 1;
        reply.updated_at = now();
        tx.save_record(&reply)?;
        let mut notices = vec![];
        if fresh {
            let data = json!({"reply_id":reply.id,"public":true});
            let seq = tx.event(&reply.run_id,Some(&reply.agent_id),"model.text.started",data.clone())?;
            notices.push(json!({"seq":seq,"run_id":reply.run_id,"agent_id":reply.agent_id,"kind":"model.text.started","data":data}));
        }
        if reply.state != ReplyState::Streaming {
            let data = json!({"reply_id":reply.id,"text":reply.text,"truncated":reply.truncated,"public":true});
            let seq = tx.event(&reply.run_id,Some(&reply.agent_id),"model.text",data.clone())?;
            notices.push(json!({"seq":seq,"run_id":reply.run_id,"agent_id":reply.agent_id,"kind":"model.text","data":data}));
        }
        Ok(notices)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop::workspace::{WorkspaceRequest, read};

    #[test]
    fn partial_snapshots_survive_reopen_and_finalize_once_without_token_events() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let store = Store::open(directory.path())?;
        let run = crate::coordinator::Coordinator::new(store.clone()).create_run(
            &std::env::current_exe()?,
            "fixture",
            Budget::default(),
            json!({}),
        )?;
        store
            .claim_lease(&run.id, "test-controller", 60)?
            .context("lease unavailable")?;
        let agent: AgentMetadata = store.load(&run.root_id)?;
        let key = PublicReply::for_request(&agent);
        publish(&store, &key, "Reading", ReplyState::Streaming)?;
        let request = WorkspaceRequest {
            run: Some(run.id.clone()),
            ..Default::default()
        };
        let first = read(&store, 1, &request)?;
        assert_eq!(
            first.data.unwrap().transcript[0].state,
            Some(ReplyState::Streaming)
        );
        for i in 0..12 {
            publish(&store, &key, &format!("Reading {i}"), ReplyState::Streaming)?;
        }
        assert_eq!(
            store
                .events(&run.id, 0, 100)?
                .iter()
                .filter(|event| event.kind.starts_with("model.text"))
                .count(),
            1
        );
        let reopened = Store::open(directory.path())?;
        assert_eq!(reopened.load::<PublicReply>(&key.id)?.text, "Reading 11");
        publish(&store, &key, "Read the exact target.", ReplyState::Complete)?;
        // Final text and earlier start events project one stable message.
        let data = read(
            &reopened,
            1,
            &WorkspaceRequest {
                known_revision: Some(first.revision),
                ..request.clone()
            },
        )?
        .data
        .unwrap();
        assert_eq!(data.transcript.len(), 1);
        assert_eq!(data.transcript[0].id, key.id);
        assert_eq!(data.transcript[0].state, Some(ReplyState::Complete));
        publish(&store, &key, "late fragment", ReplyState::Streaming)?;
        assert_eq!(
            store.load::<PublicReply>(&key.id)?.text,
            "Read the exact target."
        );
        assert_eq!(
            store
                .events(&run.id, 0, 100)?
                .iter()
                .filter(|event| event.kind.starts_with("model.text"))
                .count(),
            2
        );
        Ok(())
    }

    #[test]
    fn bounded_text_is_interrupted_on_lease_expiry_and_rejects_old_generations() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let store = Store::open(directory.path())?;
        let run = crate::coordinator::Coordinator::new(store.clone()).create_run(
            &std::env::current_exe()?,
            "fixture",
            Budget::default(),
            json!({}),
        )?;
        store
            .claim_lease(&run.id, "test-controller", 60)?
            .context("lease unavailable")?;
        let mut agent: AgentMetadata = store.load(&run.root_id)?;
        let key = PublicReply::for_request(&agent);
        publish(
            &store,
            &key,
            &"界".repeat(MAX_PUBLIC_CHARS + 1),
            ReplyState::Streaming,
        )?;
        let saved: PublicReply = store.load(&key.id)?;
        assert_eq!(saved.text.chars().count(), MAX_PUBLIC_CHARS);
        assert!(saved.truncated);
        let request = WorkspaceRequest {
            run: Some(run.id.clone()),
            ..Default::default()
        };
        let live = read(&store, 1, &request)?;
        store.transaction(|tx| {
            tx.execute("UPDATE leases SET expires_at=0", [])?;
            Ok(())
        })?;
        // Lease state participates in the revision even with no new model event.
        let data = read(
            &store,
            1,
            &WorkspaceRequest {
                known_revision: Some(live.revision),
                ..request
            },
        )?
        .data
        .unwrap();
        assert_eq!(data.transcript[0].state, Some(ReplyState::Interrupted));
        agent.generation += 1;
        store.save_record(&agent)?;
        publish(&store, &key, "old completion", ReplyState::Complete)?;
        assert_eq!(store.load::<PublicReply>(&key.id)?.revision, saved.revision);
        let next = PublicReply::for_request(&agent);
        assert_ne!(next.id, key.id);
        agent.turns += 1;
        store.save_record(&agent)?;
        publish(&store, &next, "old turn", ReplyState::Streaming)?;
        assert!(store.get_record::<PublicReply>(&next.id)?.is_none());
        Ok(())
    }
}
