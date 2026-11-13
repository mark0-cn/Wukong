//! Journal admission and completion for the owned debugger host. No pipe wait
//! holds the resource/input locks; upstream owns execution scheduling.
use super::*;
use crate::{
    resource_journal::{MutationKind, Ticket},
    session_runtime,
    session_wire::{self, request::Operation},
};

pub(crate) enum Dispatch {
    Ready(Value),
    Debug(Box<Pending>),
}

pub(crate) struct Pending {
    resource: String,
    ticket: Option<Ticket>,
    close: bool,
    exchange: session_runtime::Pending,
}
pub(crate) struct Completed {
    resource: String,
    ticket: Option<Ticket>,
    close: bool,
    exchange: session_runtime::Completed,
}
impl Pending {
    pub(crate) async fn exchange(self) -> Completed {
        Completed {
            resource: self.resource,
            ticket: self.ticket,
            close: self.close,
            exchange: self.exchange.exchange().await,
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Start {
    mode: String,
    #[serde(default = "auto")]
    engine: String,
    target: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    dump: Option<String>,
    input: Option<String>,
    launch_spec: Option<String>,
}
fn auto() -> String {
    "auto".into()
}
fn clean(params: &Map<String, Value>) -> Map<String, Value> {
    let mut params = params.clone();
    for key in ["_binding", "_lease", "request_id", "call_id"] {
        params.remove(key);
    }
    params.retain(|_, v| !v.is_null());
    params
}
fn request_id(params: &Map<String, Value>) -> Result<&str> {
    let id = params
        .get("request_id")
        .and_then(Value::as_str)
        .context("native debugger requires a stable request_id")?;
    ensure!(
        params
            .get("call_id")
            .is_none_or(|call| call.as_str() == Some(id)),
        "native call_id and request_id differ"
    );
    Ok(id)
}
fn cached(reservation: Reservation) -> Result<Value> {
    match reservation {
        Reservation::Completed {
            outcome: Outcome::Result { value },
            ..
        } => Ok(value),
        Reservation::Completed { .. } => {
            anyhow::bail!("native debugger outcome is unknown after cleanup; it cannot be replayed")
        }
        _ => anyhow::bail!(
            "native debugger request is pending or uncertain; reconcile the original resource"
        ),
    }
}
impl Runtime {
    pub(crate) fn prepare_rpc(
        &mut self,
        journal: &mut Journal,
        inputs: Option<&Inputs>,
        owner: &Owner,
        method: &str,
        params: &Map<String, Value>,
    ) -> Result<Dispatch> {
        if !method.starts_with("debug.") {
            return self
                .call(journal, inputs, owner, method, params)
                .map(Dispatch::Ready);
        }
        self.poll(journal)?;
        if method == "debug.start" {
            return self.start_debugger(
                journal,
                inputs.context("native debugger input store missing")?,
                owner,
                params,
            );
        }
        if matches!(method, "debug.control" | "debug.close") {
            request_id(params)?;
        }
        let mut fields = clean(params);
        let id = fields
            .remove("session_id")
            .and_then(|v| v.as_str().map(str::to_owned))
            .context("native debugger session_id missing")?;
        let record = journal.resource(owner, &id)?;
        ensure!(
            record.kind == ResourceKind::Debugger,
            "native resource is not a debugger"
        );
        let operation = match method {
            "debug.inspect" => {
                let query: session_wire::Inspect =
                    serde_json::from_value(Value::Object(fields.clone()))?;
                let kind = query.validate()?;
                if kind == session_wire::Inspection::State && record.phase == Phase::Closed {
                    return self
                        .debugger_terminal(&record, &AclPolicy::private_vm())
                        .map(Dispatch::Ready);
                }
                Operation::Inspect(query)
            }
            "debug.control" => {
                ensure!(
                    record.specification["debugger"]["mode"] != "dump",
                    "dump sessions permit inspection and close only"
                );
                let control: session_wire::Control =
                    serde_json::from_value(Value::Object(fields.clone()))?;
                control.validate()?;
                Operation::Control(control)
            }
            "debug.close" => {
                ensure!(
                    fields.is_empty(),
                    "native debugger close accepts only session_id"
                );
                Operation::Close(session_wire::Close {})
            }
            _ => anyhow::bail!("unsupported native debugger method"),
        };
        if method == "debug.close"
            && self
                .debugger
                .as_ref()
                .is_some_and(|live| live.request.target.resource_id == id && live.busy())
        {
            self.cancel(&id)?;
            self.debugger.as_mut().unwrap().request_cancel();
            journal.start_closing(&id)?;
            anyhow::bail!("native debugger cancellation requested; owned cleanup is still pending");
        }
        ensure!(
            !self
                .debugger
                .as_ref()
                .is_some_and(|live| live.request.target.resource_id == id && live.busy()),
            "native debugger already has an in-flight exchange"
        );
        let ticket = if method == "debug.inspect" {
            None
        } else {
            let reservation = journal.reserve_mutation(
                owner,
                request_id(params)?,
                &id,
                MutationKind::DebugControl,
                &json!({"method":method,"params":fields}),
            )?;
            let Reservation::Reserved { effect, .. } = reservation else {
                return cached(reservation).map(Dispatch::Ready);
            };
            Some(journal.begin(&effect)?)
        };
        ensure!(
            record.phase == Phase::Live,
            "native debugger is not live; reconcile its original resource"
        );
        let live = self
            .debugger
            .as_mut()
            .filter(|live| live.request.target.resource_id == id)
            .context("native debugger host ownership was interrupted")?;
        let exchange = match live.prepare(operation) {
            Ok(exchange) => exchange,
            Err(error) => {
                live.failed = Some(format!("{error:#}").chars().take(2048).collect());
                return Err(error);
            }
        };
        if method == "debug.close" {
            journal.start_closing(&id)?;
        }
        Ok(Dispatch::Debug(Box::new(Pending {
            resource: id,
            ticket,
            close: method == "debug.close",
            exchange,
        })))
    }
    /// Read only after journal-confirmed cleanup. The same run's successor can
    /// inspect original ownership; it cannot turn this projection into a start.
    pub(super) fn debugger_terminal(
        &self,
        record: &ResourceRecord,
        policy: &AclPolicy,
    ) -> Result<Value> {
        ensure!(
            record.kind == ResourceKind::Debugger && record.phase == Phase::Closed,
            "debugger terminal observation requires confirmed closure"
        );
        let status_path = self.result_path(&record.id, "status.json");
        if status_path.try_exists()? {
            policy.verify(&status_path, false)?;
            let value: Value =
                serde_json::from_slice(&deployment::read_bytes(&status_path, 65536)?)?;
            ensure!(
                value["session_id"] == record.id
                    && value["worker_session_id"] == record.id
                    && value["native_owner"] == serde_json::to_value(&record.owner)?
                    && value["kind"] == "debugger"
                    && value["production_execution"] == false
                    && value["state"] == "lost"
                    && value["cleanup_pending"] == false
                    && value["result"]["outcome"] == "unknown"
                    && value["result"]["jobs_empty"] == true
                    && value["result"]["profiles_removed"] == true
                    && value["result"]["scratch_removed"] == true,
                "native recovered debugger status binding or cleanup differs"
            );
            let artifact = value["result"]["recovery_artifact"]
                .as_str()
                .context("debugger recovery artifact missing")?;
            ensure!(
                artifact.starts_with("recovery-"),
                "debugger recovery selector is not a recovery receipt"
            );
            let proof_path = self.result_path(&record.id, &artifact_suffix(artifact)?);
            policy.verify(&proof_path, false)?;
            let bytes = deployment::read_bytes(&proof_path, 65536)?;
            let proof: Value = serde_json::from_slice(&bytes)?;
            ensure!(
                value["result"]["recovery_receipt_sha256"] == hex::encode(Sha256::digest(&bytes))
                    && proof["schema"] == crate::resource_process::SCHEMA
                    && proof["scope"] == "owned_resource_recovery"
                    && proof["resource_id"] == record.id
                    && proof["owner"] == serde_json::to_value(&record.owner)?
                    && proof["jobs_empty"] == true
                    && proof["profiles_removed"] == true
                    && proof["scratch_removed"] == true,
                "debugger recovery receipt content or binding differs"
            );
            return Ok(value);
        }
        let proof_path = self.result_path(&record.id, "debug-result.json");
        policy.verify(&proof_path, false)?;
        let proof: Value = serde_json::from_slice(&deployment::read_bytes(&proof_path, 65536)?)?;
        ensure!(
            proof["schema"] == crate::session_host::SCHEMA
                && proof["scope"] == "native_debugger_host"
                && proof["resource_id"] == record.id
                && proof["owner"] == serde_json::to_value(&record.owner)?
                && proof["outcome"] == "closed"
                && proof["jobs_empty"] == true
                && proof["profiles_removed"] == true
                && proof["scratch_removed"] == true,
            "native closed debugger proof binding or cleanup differs"
        );
        Ok(
            json!({"session_id":record.id,"worker_session_id":record.id,"native_owner":record.owner,
            "kind":"debugger","state":"closed","cleanup_pending":false,"result":proof,"production_execution":false}),
        )
    }
    fn start_debugger(
        &mut self,
        journal: &mut Journal,
        inputs: &Inputs,
        owner: &Owner,
        params: &Map<String, Value>,
    ) -> Result<Dispatch> {
        let request_id = request_id(params)?;
        let start: Start = serde_json::from_value(Value::Object(clean(params)))?;
        ensure!(
            matches!(start.mode.as_str(), "launch" | "dump")
                && matches!(start.engine.as_str(), "auto" | "dbgeng")
                && (start.mode == "dump") == start.dump.is_some(),
            "native debugger requires launch or dump with auto/dbgeng; only dump mode requires a dump"
        );
        ensure!(
            start.launch_spec.is_some() || start.target.is_some(),
            "native debugger requires a target or LaunchSpec"
        );
        ensure!(
            start.launch_spec.is_none() || start.target.is_none() && start.args.is_empty(),
            "LaunchSpec owns debugger target and arguments"
        );
        ensure!(
            start.mode != "dump" || start.input.is_none() && start.args.is_empty(),
            "dump inspection cannot supply execution arguments or stdin"
        );
        let spec = Specification {
            argv: start
                .target
                .iter()
                .cloned()
                .chain(start.args.iter().cloned())
                .collect(),
            launch_spec: start.launch_spec.clone(),
            input: start.input.clone(),
            env: BTreeMap::new(),
            cwd: default_cwd(),
            timeout_secs: start.launch_spec.is_none().then_some(3600),
            output_limit: default_output(),
        };
        spec.validate()?;
        let specification = json!({"native_debugger_schema":crate::session_host::SCHEMA,"debugger":start,"process":spec});
        if let Some(existing) =
            journal.lookup_start(owner, request_id, ResourceKind::Debugger, &specification)?
        {
            if matches!(
                &existing,
                Reservation::Completed {
                    phase: Phase::Closed,
                    ..
                }
            ) {
                anyhow::bail!("native debugger start is already closed and cannot be replayed");
            }
            if let Reservation::Completed {
                resource, phase, ..
            } = &existing
            {
                ensure!(
                    *phase == Phase::Live
                        && self.debugger.as_ref().is_some_and(|live| live
                            .request
                            .target
                            .resource_id
                            == *resource
                            && !live.needs_recovery()),
                    "native debugger start was completed but host ownership is unavailable; reconcile the original resource"
                );
            }
            return cached(existing).map(Dispatch::Ready);
        }
        let mut request = crate::session_request::Request {
            schema: crate::session_host::SCHEMA,
            target: spec.prepare(inputs, owner)?,
            mode: match &start.dump {
                Some(path) => crate::session_request::Mode::Dump {
                    file: inputs.reference(owner, path, Purpose::Inspect)?,
                },
                None => crate::session_request::Mode::Launch,
            },
        };
        request.validate()?;
        ensure!(
            self.running.is_none() && self.debugger.is_none() && self.recovery.is_none(),
            "native host remains owned"
        );
        let reserved =
            journal.reserve_start(owner, request_id, ResourceKind::Debugger, &specification)?;
        let Reservation::Reserved {
            effect, resource, ..
        } = reserved
        else {
            return cached(reserved).map(Dispatch::Ready);
        };
        request.target.resource_id = resource.clone();
        let ticket = journal.begin(&effect)?;
        self.debugger = Some(session_runtime::Live::spawn(
            &self.root,
            &self.executable,
            request,
        )?);
        let exchange = self
            .debugger
            .as_mut()
            .unwrap()
            .prepare(Operation::Open(session_wire::Open {}))?;
        Ok(Dispatch::Debug(Box::new(Pending {
            resource,
            ticket: Some(ticket),
            close: false,
            exchange,
        })))
    }
    pub(crate) fn complete_debugger(
        &mut self,
        journal: &mut Journal,
        completed: Completed,
    ) -> Result<Value> {
        let live = self
            .debugger
            .as_mut()
            .filter(|live| live.request.target.resource_id == completed.resource)
            .context("native debugger completion lost its original host; reconcile the journal")?;
        let result = (|| -> Result<Value> {
            let value = live.complete(completed.exchange)?;
            let record = journal.resource(&live.request.target.owner, &completed.resource)?;
            ensure!(
                if completed.close {
                    record.phase == Phase::Closing
                } else {
                    matches!(record.phase, Phase::Preparing | Phase::Live)
                },
                "native debugger was cancelled while its operation was in flight"
            );
            ensure!(
                value["session_id"] == completed.resource
                    && value["worker_session_id"] == completed.resource
                    && value["native_owner"] == serde_json::to_value(&live.request.target.owner)?,
                "native debugger reply ownership differs"
            );
            ensure!(
                value["state"] == if completed.close { "closed" } else { "running" },
                "native debugger reply lifecycle differs"
            );
            if completed.close {
                live.finish()?;
                let path = self
                    .root
                    .join("metadata/resource-results")
                    .join(format!("{}.debug-result.json", completed.resource));
                AclPolicy::private_vm().verify(&path, false)?;
                let proof: Value = serde_json::from_slice(&deployment::read_bytes(&path, 65536)?)?;
                ensure!(
                    proof == value["response"]["cleanup"]
                        && proof["schema"] == crate::session_host::SCHEMA
                        && proof["scope"] == "native_debugger_host"
                        && proof["owner"] == serde_json::to_value(&live.request.target.owner)?
                        && proof["resource_id"] == completed.resource
                        && proof["request_sha256"] == live.request_hash
                        && proof["read_only"] == live.request.read_only()
                        && proof["target_executed"] == !live.request.read_only()
                        && proof["outcome"] == "closed"
                        && proof["jobs_empty"] == true
                        && proof["profiles_removed"] == true
                        && proof["scratch_removed"] == true,
                    "native debugger cleanup proof differs"
                );
            }
            if let Some(ticket) = &completed.ticket {
                journal.complete(
                    ticket,
                    &Outcome::Result {
                        value: value.clone(),
                    },
                )?;
            }
            if completed.close {
                journal.close(
                    &completed.resource,
                    Cleanup {
                        jobs_empty: true,
                        profiles_removed: true,
                        scratch_removed: true,
                    },
                )?;
            }
            Ok(value)
        })();
        match &result {
            Ok(_) if completed.close => {
                self.debugger = None;
            }
            Err(error) => {
                live.failed = Some(format!("{error:#}").chars().take(2048).collect());
                self.cancel(&completed.resource)?;
            }
            _ => {}
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn owner() -> Owner {
        Owner {
            identity: crate::lease_proxy::Identity {
                protocol: 2,
                installation_id: "a".repeat(32),
                boot_id: format!("windows:{}:1", "b".repeat(32)),
                package_sha256: "c".repeat(64),
            },
            run_id: "run".into(),
            owner_id: "controller".into(),
            epoch: 1,
        }
    }
    #[test]
    fn debugger_start_reconciles_prior_intent_before_resolving_new_inputs() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let owner = owner();
        let policy = AclPolicy::current_user()?;
        policy.apply(directory.path(), true)?;
        let mut journal =
            Journal::with_policy(directory.path(), owner.identity.clone(), policy.clone())?;
        let inputs = Inputs::open(directory.path(), policy)?;
        let mut runtime = Runtime {
            root: directory.path().into(),
            executable: directory.path().join("never-execute"),
            installation: owner.identity.installation_id.clone(),
            running: None,
            recovery: None,
            debugger: None,
        };
        for params in [
            json!({"request_id":"invalid","mode":"launch","args":["files/not-a-target.exe"]}),
            json!({"request_id":"invalid","mode":"dump","target":"files/image.exe","dump":"files/image.dmp"}),
            json!({"request_id":"invalid","mode":"launch","engine":"gdb_mi","target":"files/image.exe"}),
            json!({"request_id":"invalid","mode":"launch","launch_spec":"files/launch.json","target":"files/image.exe"}),
        ] {
            assert!(
                runtime
                    .start_debugger(&mut journal, &inputs, &owner, params.as_object().unwrap())
                    .is_err()
            );
            assert!(journal.idle()?);
        }
        let params = json!({"request_id":"stable","mode":"launch","target":"files/image.exe"});
        let start: Start = serde_json::from_value(clean(params.as_object().unwrap()).into())?;
        let process: Specification =
            serde_json::from_value(json!({"argv":["files/image.exe"],"timeout_secs":3600}))?;
        let spec = json!({"native_debugger_schema":crate::session_host::SCHEMA,"debugger":start,"process":process});
        let Reservation::Reserved {
            effect, resource, ..
        } = journal.reserve_start(&owner, "stable", ResourceKind::Debugger, &spec)?
        else {
            anyhow::bail!("reservation absent")
        };
        let _ticket = journal.begin(&effect)?;
        let mut successor = owner.clone();
        successor.owner_id = "successor".into();
        successor.epoch = 2;
        let error = match runtime.start_debugger(
            &mut journal,
            &inputs,
            &successor,
            params.as_object().unwrap(),
        ) {
            Err(error) => error,
            Ok(_) => anyhow::bail!("replayed interrupted start"),
        };
        assert!(error.to_string().contains("pending or uncertain"));
        assert!(!journal.idle()?);
        assert!(journal.begin(&effect).is_err());
        journal.start_closing(&resource)?;
        journal.close(
            &resource,
            Cleanup {
                jobs_empty: true,
                profiles_removed: true,
                scratch_removed: true,
            },
        )?;
        assert!(journal.idle()?);
        let error = match runtime.start_debugger(
            &mut journal,
            &inputs,
            &successor,
            params.as_object().unwrap(),
        ) {
            Err(error) => error,
            Ok(_) => anyhow::bail!("replayed closed start"),
        };
        assert!(error.to_string().contains("already closed"));
        let mut changed = params;
        changed["target"] = json!("files/changed.exe");
        let error = match runtime.start_debugger(
            &mut journal,
            &inputs,
            &successor,
            changed.as_object().unwrap(),
        ) {
            Err(error) => error,
            Ok(_) => anyhow::bail!("changed original intent"),
        };
        assert!(!error.to_string().contains("native input"));
        assert!(journal.idle()?);
        Ok(())
    }
    #[test]
    fn terminal_debugger_observation_preserves_unknown_outcomes_and_binds_receipt_bytes()
    -> Result<()> {
        let directory = tempfile::tempdir()?;
        let policy = AclPolicy::current_user()?;
        policy.apply(directory.path(), true)?;
        policy.create_directory(&directory.path().join("metadata"))?;
        policy.create_directory(&directory.path().join("metadata/resource-results"))?;
        let runtime = Runtime {
            root: directory.path().into(),
            executable: directory.path().join("never-execute"),
            installation: owner().identity.installation_id,
            running: None,
            recovery: None,
            debugger: None,
        };
        let record = ResourceRecord {
            id: "d".repeat(32),
            phase: Phase::Closed,
            start_effect: "e".repeat(64),
            owner: owner(),
            kind: ResourceKind::Debugger,
            specification: json!({}),
        };
        let artifact = format!("recovery-{}", "f".repeat(32));
        let proof = json!({"schema":crate::resource_process::SCHEMA,"scope":"owned_resource_recovery",
            "resource_id":record.id,"owner":record.owner,"jobs_empty":true,"profiles_removed":true,"scratch_removed":true});
        let proof_path = runtime.result_path(&record.id, &artifact_suffix(&artifact)?);
        deployment::publish_json(&proof_path, &proof, &policy)?;
        let original = fs::read(&proof_path)?;
        let status = json!({"session_id":record.id,"worker_session_id":record.id,"native_owner":record.owner,"kind":"debugger","production_execution":false,
            "state":"lost","cleanup_pending":false,"result":{"outcome":"unknown","jobs_empty":true,
            "profiles_removed":true,"scratch_removed":true,"recovery_artifact":artifact,
            "recovery_receipt_sha256":hex::encode(Sha256::digest(&original))}});
        let status_path = runtime.result_path(&record.id, "status.json");
        deployment::publish_json(&status_path, &status, &policy)?;
        assert_eq!(runtime.debugger_terminal(&record, &policy)?, status);
        let mut foreign_epoch = record.clone();
        foreign_epoch.owner.epoch += 1;
        let mut foreign_run = record.clone();
        foreign_run.owner.run_id = "another-run".into();
        let mut foreign_resource = record.clone();
        foreign_resource.id = "0".repeat(32);
        for changed in [&foreign_epoch, &foreign_run, &foreign_resource] {
            assert!(runtime.debugger_terminal(changed, &policy).is_err());
        }
        let mut live = record.clone();
        live.phase = Phase::Closing;
        assert!(runtime.debugger_terminal(&live, &policy).is_err());
        let mut process = record.clone();
        process.kind = ResourceKind::Process;
        assert!(runtime.debugger_terminal(&process, &policy).is_err());
        let mut altered = proof.clone();
        altered["owner"]["epoch"] = json!(99);
        fs::write(&proof_path, serde_json::to_vec(&altered)?)?;
        assert!(runtime.debugger_terminal(&record, &policy).is_err());
        fs::write(&proof_path, &original)?;
        for (field, value) in [
            ("outcome", json!("closed")),
            ("jobs_empty", json!(false)),
            ("recovery_artifact", json!("stdout")),
            ("recovery_artifact", json!("recovery-../other")),
        ] {
            let mut changed = status.clone();
            changed["result"][field] = value;
            fs::write(&status_path, serde_json::to_vec(&changed)?)?;
            assert!(
                runtime.debugger_terminal(&record, &policy).is_err(),
                "{field}"
            );
        }
        fs::remove_file(&status_path)?;
        assert!(runtime.debugger_terminal(&record, &policy).is_err());
        let clean = json!({"schema":crate::session_host::SCHEMA,"scope":"native_debugger_host",
            "resource_id":record.id,"owner":record.owner,"outcome":"closed",
            "jobs_empty":true,"profiles_removed":true,"scratch_removed":true});
        deployment::publish_json(
            &runtime.result_path(&record.id, "debug-result.json"),
            &clean,
            &policy,
        )?;
        let closed = runtime.debugger_terminal(&record, &policy)?;
        assert_eq!(closed["state"], "closed");
        assert_eq!(closed["result"], clean);
        assert_eq!(fs::read(&proof_path)?, original);
        Ok(())
    }
}
