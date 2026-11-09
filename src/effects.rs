//! Durable admission, receipts and recovery decisions for business effects.
use crate::{
    domain::*,
    runtime::protocol::{TransportFailure, WorkerError, WorkerErrorKind},
    store::Store,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectOutcome {
    pub value: Value,
    pub is_error: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReplayPolicy {
    ReadOnly,
    Transactional,
    WorkerIdempotent,
    OperatorOnly,
}
impl ReplayPolicy {
    pub(crate) fn side_effecting(self) -> bool {
        self != Self::ReadOnly
    }
    pub(crate) fn interrupted_state(self) -> CallState {
        if self.side_effecting() {
            CallState::Uncertain
        } else {
            CallState::Failed
        }
    }
}
pub(crate) enum Admission {
    Execute(ToolCallRecord),
    Replay(EffectOutcome),
}

#[derive(Debug)]
pub(crate) struct WorkerDispatch {
    pub method: String,
    pub request_id: String,
}
impl std::fmt::Display for WorkerDispatch {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            out,
            "worker dispatch {}; reconcile stable request {} before changing its arguments",
            self.method, self.request_id
        )?;
        if self.method.starts_with("debug.") || self.method == "interact.start" {
            write!(
                out,
                " (this is a stateful session: to re-run with different arguments, close the session and open a new one instead of re-issuing the same request)"
            )?;
        }
        Ok(())
    }
}
impl std::error::Error for WorkerDispatch {}

#[derive(Serialize, Deserialize)]
struct WorkerReceipt {
    method: String,
    arguments_sha256: String,
    result: Value,
}

fn arguments_sha256(arguments: &Value) -> Result<String> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(arguments)?)))
}

pub(crate) struct DurableEffects<'a> {
    store: &'a Store,
    limit: usize,
}
impl<'a> DurableEffects<'a> {
    pub(crate) fn new(store: &'a Store, limit: usize) -> Self {
        Self { store, limit }
    }
    pub(crate) fn call_id(agent: &AgentMetadata, wire: &str) -> String {
        format!(
            "{}:g{}:t{}:{}",
            agent.id, agent.generation, agent.turns, wire
        )
    }
    pub(crate) fn worker_request_id(agent: &AgentMetadata, call: &str) -> String {
        format!(
            "request_{}",
            hex::encode(Sha256::digest(
                format!("{}:{}:{call}", agent.run_id, agent.id).as_bytes()
            ))
        )
    }
    pub(crate) fn admit(
        &self,
        agent: &AgentMetadata,
        call: &str,
        name: &str,
        args: &Value,
        policy: ReplayPolicy,
    ) -> Result<Admission> {
        ensure!(
            !call.is_empty() && call.len() <= 512,
            "invalid tool call ID"
        );
        enum Decision {
            Execute(ToolCallRecord),
            Replay(EffectOutcome),
            Finish(ToolCallRecord, Value, bool, CallState),
        }
        let decision = self.store.write(|tx| {
            let existing: Option<ToolCallRecord> = tx.get_record(call)?;
            if let Some(existing) = &existing {
                ensure!(
                    existing.agent_id == agent.id
                        && existing.run_id == agent.run_id
                        && existing.name == name
                        && existing.arguments == *args,
                    "tool call ID reused with a different request"
                );
                if matches!(existing.state, CallState::Completed | CallState::Failed) {
                    return Ok(Decision::Replay(serde_json::from_value(
                        existing.result.clone().context("completed call result missing")?,
                    )?));
                }
                // The domain transition can have committed even when the final
                // envelope was lost. Settle it before checking agent lifecycle.
                if let Some(receipt) = tx.get::<Value>("tool_receipt", call)? {
                    return Ok(Decision::Finish(existing.clone(), receipt, false, CallState::Completed));
                }
                if policy == ReplayPolicy::OperatorOnly {
                    if let Some(result) = &existing.result {
                        return Ok(Decision::Replay(serde_json::from_value(result.clone())?));
                    }
                    let raw = json!({"error":"prior MCP side effect outcome is unknown; operator reconciliation is required before any retry","call_id":call});
                    return Ok(Decision::Finish(existing.clone(), raw, true, CallState::Uncertain));
                }
            }
            let mut record = existing.unwrap_or(ToolCallRecord {
                id: call.into(),
                run_id: agent.run_id.clone(),
                agent_id: agent.id.clone(),
                name: name.into(),
                arguments: args.clone(),
                state: CallState::Prepared,
                side_effecting: policy.side_effecting(),
                result: None,
                evidence_id: None,
                created_at: now(),
                updated_at: now(),
            });
            record.state = CallState::Running;
            record.updated_at = now();
            tx.save_record(&record)?;
            tx.event(&agent.run_id, Some(&agent.id), "tool.started", json!({"call_id":call,"name":name}))?;
            Ok(Decision::Execute(record))
        })?;
        Ok(match decision {
            Decision::Execute(record) => Admission::Execute(record),
            Decision::Replay(outcome) => Admission::Replay(outcome),
            Decision::Finish(record, raw, error, state) => {
                Admission::Replay(self.finish(record, raw, error, state)?)
            }
        })
    }
    pub(crate) fn finish(
        &self,
        record: ToolCallRecord,
        raw: Value,
        is_error: bool,
        state: CallState,
    ) -> Result<EffectOutcome> {
        finish_record(self.store, self.limit, record, raw, is_error, state)
    }
    pub(crate) fn commit(
        &self,
        agent: &AgentMetadata,
        call: &str,
        action: impl FnOnce(&crate::store::StateSession<'_>) -> Result<Value>,
    ) -> Result<Value> {
        self.store.write(|tx| {
            if let Some(receipt) = tx.get("tool_receipt", call)? {
                return Ok(receipt);
            }
            let value = action(tx)?;
            tx.put("tool_receipt", call, &agent.run_id, &value)?;
            Ok(value)
        })
    }
    pub(crate) fn replace_result(&self, call: &str, value: &Value, is_error: bool) -> Result<()> {
        self.store.write(|tx| {
            let mut record: ToolCallRecord = tx.get_record(call)?.context("tool call missing")?;
            ensure!(
                matches!(record.state, CallState::Completed | CallState::Failed),
                "only settled call results may be replaced"
            );
            record.result = Some(serde_json::to_value(EffectOutcome {
                value: value.clone(),
                is_error,
            })?);
            record.updated_at = now();
            tx.save_record(&record)
        })
    }
    pub(crate) fn worker_reply(
        &self,
        agent: &AgentMetadata,
        call: &str,
        method: &str,
        args: &Value,
    ) -> Result<Option<Value>> {
        let receipt: Option<WorkerReceipt> = self
            .store
            .get("worker_receipt", &Self::worker_request_id(agent, call))?;
        receipt
            .map(|receipt| {
                ensure!(
                    receipt.method == method && receipt.arguments_sha256 == arguments_sha256(args)?,
                    "worker request ID reused with a different request"
                );
                Ok(receipt.result)
            })
            .transpose()
    }
    pub(crate) fn record_worker_reply(
        &self,
        agent: &AgentMetadata,
        call: &str,
        method: &str,
        args: Value,
        result: Value,
    ) -> Result<()> {
        let id = Self::worker_request_id(agent, call);
        let arguments_sha256 = arguments_sha256(&args)?;
        self.store.write(|tx| {
            if let Some(previous) = tx.get::<WorkerReceipt>("worker_receipt", &id)? {
                ensure!(
                    previous.method == method
                        && previous.arguments_sha256 == arguments_sha256
                        && previous.result == result,
                    "conflicting worker receipt"
                );
                return Ok(());
            }
            tx.put(
                "worker_receipt",
                &id,
                &agent.run_id,
                &WorkerReceipt {
                    method: method.into(),
                    arguments_sha256,
                    result,
                },
            )
        })
    }
    pub(crate) fn failure_state(
        &self,
        policy: ReplayPolicy,
        agent: &AgentMetadata,
        call: &str,
        error: &anyhow::Error,
    ) -> Result<CallState> {
        if policy == ReplayPolicy::ReadOnly {
            return Ok(CallState::Failed);
        }
        if policy == ReplayPolicy::OperatorOnly
            || self
                .store
                .read(|tx| tx.contains("worker_receipt", &Self::worker_request_id(agent, call)))?
        {
            return Ok(CallState::Uncertain);
        }
        if let Some(error) = error.downcast_ref::<WorkerError>() {
            return Ok(
                if matches!(
                    error.kind,
                    WorkerErrorKind::InvalidRequest
                        | WorkerErrorKind::SessionLost
                        | WorkerErrorKind::Conflict
                ) {
                    CallState::Failed
                } else {
                    CallState::Uncertain
                },
            );
        }
        Ok(
            if error.downcast_ref::<TransportFailure>().is_some()
                || error.downcast_ref::<WorkerDispatch>().is_some()
            {
                CallState::Uncertain
            } else {
                CallState::Failed
            },
        )
    }
}

fn finish_record(
    store: &Store,
    limit: usize,
    mut record: ToolCallRecord,
    raw: Value,
    is_error: bool,
    state: CallState,
) -> Result<EffectOutcome> {
    let bytes = serde_json::to_vec(&raw)?;
    let run: RunRecord = store.load(&record.run_id)?;
    let evidence=store.artifact(&record.run_id,&record.agent_id,&record.id,&record.name,&bytes,"application/json",json!({"raw_tool_output":true,"tool":record.name,"is_error":is_error,"target_sha256":run.target_sha256,"tool_protocol_version":1}))?;
    let envelope_overhead = serde_json::to_vec(&json!({"result":null,"evidence_id":evidence.id,"truncated":false,"total_bytes":bytes.len()}))?.len().saturating_sub(4);
    let truncated = bytes.len().saturating_add(envelope_overhead) > limit;
    let result = if truncated {
        let mut summary = json!({"preview":prefix_utf8(&bytes,limit.saturating_sub(1024)),"retrieve":"use evidence_read with byte ranges"});
        for key in [
            "id",
            "job_id",
            "session_id",
            "state",
            "status",
            "path",
            "sha256",
            "finding_id",
            "verification_id",
            "parked",
            "wake_at",
        ] {
            if let Some(value) = raw.get(key) {
                let value = match value {
                    Value::String(value) => Some(json!(prefix_utf8(value.as_bytes(), 128))),
                    Value::Bool(_) | Value::Number(_) | Value::Null => Some(value.clone()),
                    _ => None,
                };
                if let Some(value) = value {
                    summary[key] = value;
                }
            }
        }
        summary
    } else {
        raw
    };
    let mut outcome = EffectOutcome {
        value: json!({"result":result,"evidence_id":evidence.id,"truncated":truncated,"total_bytes":bytes.len()}),
        is_error,
    };
    if truncated {
        while serde_json::to_vec(&outcome.value)?.len() > limit {
            let preview = outcome.value["result"]["preview"]
                .as_str()
                .unwrap_or_default();
            if !preview.is_empty() {
                outcome.value["result"]["preview"] =
                    json!(prefix_utf8(preview.as_bytes(), preview.len() / 2));
            } else {
                let summary = outcome.value["result"]
                    .as_object_mut()
                    .context("bounded result is not an object")?;
                let key = summary
                    .keys()
                    .find(|key| !matches!(key.as_str(), "preview" | "retrieve"))
                    .cloned()
                    .context(
                        "configured tool output limit is too small for the evidence envelope",
                    )?;
                summary.remove(&key);
            }
        }
    }
    record.state = state;
    record.result = Some(serde_json::to_value(&outcome)?);
    record.evidence_id = Some(evidence.id);
    record.updated_at = now();
    store.finish_call(&record)?;
    Ok(outcome)
}

/// Resolve an unknown effect from operator-supplied evidence without opening
/// any provider, runtime or MCP connection. A live controller lease blocks it.
pub fn reconcile_record(
    store: &Store,
    call_id: &str,
    resolution: Value,
    evidence_ids: &[String],
    is_error: bool,
) -> Result<EffectOutcome> {
    let record: ToolCallRecord = store.load(call_id)?;
    ensure!(
        matches!(record.state, CallState::Running | CallState::Uncertain),
        "only an unresolved call can be reconciled"
    );
    ensure!(
        resolution.is_object() && !evidence_ids.is_empty(),
        "operator reconciliation requires a result object and supporting evidence"
    );
    store.verify_evidence_links(&record.run_id, evidence_ids)?;
    let owner = id("reconcile");
    let token = store.claim_lease(&record.run_id, &owner, 30)?.context(
        "a live controller owns this assessment; pause it before reconciling an unknown effect",
    )?;
    let fenced = store.fenced(token.clone());
    let result = (|| {
        let store = &fenced;
        let record: ToolCallRecord = store.load(call_id)?;
        ensure!(
            matches!(record.state, CallState::Running | CallState::Uncertain),
            "only an unresolved call can be reconciled"
        );
        store.event(
            &record.run_id,
            Some(&record.agent_id),
            "tool.operator_reconciled",
            json!({"call_id":call_id,"evidence_ids":evidence_ids,"is_error":is_error}),
        )?;
        finish_record(
            store,
            12000,
            record.clone(),
            json!({"operator_reconciled":true,"result":resolution,"supporting_evidence_ids":evidence_ids}),
            is_error,
            if is_error {
                CallState::Failed
            } else {
                CallState::Completed
            },
        )
    })();
    let release = store.release_token(&token);
    match result {
        Ok(value) => {
            release?;
            Ok(value)
        }
        Err(error) => {
            let _ = release;
            Err(error)
        }
    }
}

pub(crate) fn prefix_utf8(bytes: &[u8], limit: usize) -> String {
    let mut end = bytes.len().min(limit);
    while end > 0 && std::str::from_utf8(&bytes[..end]).is_err() {
        end -= 1;
    }
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::AppConfig, coordinator::Coordinator, runtime::RuntimeBackend, tools::ToolService,
    };

    fn fixture(dir: &std::path::Path) -> Result<(Store, AgentMetadata)> {
        let target = dir.join("sample");
        std::fs::write(&target, b"fixture")?;
        let store = Store::open(dir.join("state"))?;
        let run = Coordinator::new(store.clone()).create_run(
            &target,
            "fixture",
            Budget::default(),
            json!({}),
        )?;
        let agent = Coordinator::new(store.clone())
            .create_child(
                &run.root_id,
                serde_json::from_value(
                    json!({"name":"worker","role":"analyst","task":"fixture","scope":"fixture"}),
                )?,
            )?
            .agent
            .metadata;
        Ok((store, agent))
    }

    #[test]
    fn failed_transaction_leaves_neither_mutation_nor_receipt() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let (store, agent) = fixture(dir.path())?;
        let effects = DurableEffects::new(&store, 12000);
        let result = effects.commit(&agent, "call", |tx| {
            tx.put("fixture", "change", &agent.run_id, &json!(true))?;
            anyhow::bail!("crash before committing receipt")
        });
        assert!(result.is_err());
        assert!(store.get::<Value>("fixture", "change")?.is_none());
        assert!(store.get::<Value>("tool_receipt", "call")?.is_none());
        effects.commit(&agent, "call", |tx| {
            tx.put("fixture", "change", &agent.run_id, &json!(true))?;
            Ok(json!({"committed":true}))
        })?;
        assert_eq!(
            effects.commit(&agent, "call", |_| anyhow::bail!("must not replay"))?["committed"],
            true
        );
        Ok(())
    }

    #[test]
    fn failure_classification_uses_typed_boundaries() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let (store, agent) = fixture(dir.path())?;
        let effects = DurableEffects::new(&store, 12000);
        let policy = ReplayPolicy::WorkerIdempotent;
        let local = anyhow::anyhow!("worker dispatch text is only a local validation failure");
        assert!(matches!(
            effects.failure_state(policy, &agent, "call", &local)?,
            CallState::Failed
        ));
        let transport = anyhow::Error::new(TransportFailure {
            timed_out: true,
            message: "localized message".into(),
        });
        assert!(matches!(
            effects.failure_state(policy, &agent, "call", &transport)?,
            CallState::Uncertain
        ));
        let unrecorded_reply = anyhow::anyhow!(
            "controller could not persist the successful RPC reply"
        )
        .context(WorkerDispatch {
            method: "exec.run".into(),
            request_id: "id".into(),
        });
        assert!(matches!(
            effects.failure_state(policy, &agent, "call", &unrecorded_reply)?,
            CallState::Uncertain
        ));
        let rejected = anyhow::Error::new(WorkerError {
            kind: WorkerErrorKind::InvalidRequest,
            message: "localized message".into(),
        })
        .context(WorkerDispatch {
            method: "exec.run".into(),
            request_id: "id".into(),
        });
        assert!(matches!(
            effects.failure_state(policy, &agent, "call", &rejected)?,
            CallState::Failed
        ));
        effects.record_worker_reply(
            &agent,
            "call",
            "exec.run",
            json!({}),
            json!({"exit_code":0}),
        )?;
        assert!(
            matches!(
                effects.failure_state(policy, &agent, "call", &local)?,
                CallState::Uncertain
            ),
            "post-processing failures preserve a successful worker receipt"
        );
        Ok(())
    }

    #[tokio::test]
    async fn recovery_reuses_controller_and_worker_receipts_without_repeating_effects() -> Result<()>
    {
        let dir = tempfile::tempdir()?;
        let (store, agent) = fixture(dir.path())?;
        let mut config = AppConfig::default();
        config.runtime.backend = RuntimeBackend::TrustedLocal;
        config.runtime.state_dir = dir.path().join("worker");
        let args = json!({"command":"printf x >> \"$RE_AGENT_STATE_DIR/counter\"; printf done"});
        let mut tools = ToolService::new(config.clone(), store.clone()).await?;
        let first = tools
            .execute(&agent.id, "crash-boundary", "exec_command", args.clone())
            .await?;
        assert!(!first.is_error, "{first:?}");
        for lose_controller_receipt in [false, true] {
            // The worker committed, but the controller crashed before recording
            // its final envelope (and optionally before persisting the RPC reply).
            store.write(|tx| {
                let mut call: ToolCallRecord = tx.get("call", "crash-boundary")?.unwrap();
                call.state = CallState::Running;
                call.result = None;
                call.evidence_id = None;
                tx.put("call", &call.id, &call.run_id, &call)?;
                if lose_controller_receipt {
                    tx.remove(
                        "worker_receipt",
                        &DurableEffects::worker_request_id(&agent, &call.id),
                    )?;
                }
                Ok(())
            })?;
            tools.close().await;
            tools = ToolService::new(config.clone(), Store::open(store.root())?).await?;
            let replay = tools
                .execute(&agent.id, "crash-boundary", "exec_command", args.clone())
                .await?;
            assert!(!replay.is_error, "{replay:?}");
            assert_eq!(replay.value["result"], first.value["result"]);
            assert_eq!(
                std::fs::read(config.runtime.state_dir.join("counter"))?,
                b"x"
            );
        }
        tools.runtime()?.call("worker.shutdown", json!({})).await?;
        tools.close().await;
        Ok(())
    }
}
