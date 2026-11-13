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
                query.validate()?;
                Operation::Inspect(query)
            }
            "debug.control" => {
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
                .is_some_and(|live| live.request.resource_id == id && live.busy())
        {
            self.cancel(&id)?;
            journal.start_closing(&id)?;
            anyhow::bail!("native debugger cancellation requested; owned cleanup is still pending");
        }
        ensure!(
            !self
                .debugger
                .as_ref()
                .is_some_and(|live| live.request.resource_id == id && live.busy()),
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
            .filter(|live| live.request.resource_id == id)
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
            start.mode == "launch"
                && matches!(start.engine.as_str(), "auto" | "dbgeng")
                && start.dump.is_none(),
            "native debugger currently admits launch with auto/dbgeng only"
        );
        ensure!(
            start.launch_spec.is_some() || start.target.is_some(),
            "native debugger launch requires a target or LaunchSpec"
        );
        ensure!(
            start.launch_spec.is_none() || start.target.is_none() && start.args.is_empty(),
            "LaunchSpec owns debugger target and arguments"
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
                        && self
                            .debugger
                            .as_ref()
                            .is_some_and(|live| live.request.resource_id == *resource
                                && !live.needs_recovery()),
                    "native debugger start was completed but host ownership is unavailable; reconcile the original resource"
                );
            }
            return cached(existing).map(Dispatch::Ready);
        }
        let mut request = spec.prepare(inputs, owner)?;
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
        request.resource_id = resource.clone();
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
            .filter(|live| live.request.resource_id == completed.resource)
            .context("native debugger completion lost its original host; reconcile the journal")?;
        let result = (|| -> Result<Value> {
            let value = live.complete(completed.exchange)?;
            let record = journal.resource(&live.request.owner, &completed.resource)?;
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
                    && value["native_owner"] == serde_json::to_value(&live.request.owner)?,
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
                        && proof["owner"] == serde_json::to_value(&live.request.owner)?
                        && proof["resource_id"] == completed.resource
                        && proof["request_sha256"] == live.request_hash
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
    #[test]
    fn debugger_start_reconciles_prior_intent_before_resolving_new_inputs() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let owner = Owner {
            identity: crate::lease_proxy::Identity {
                protocol: 2,
                installation_id: "a".repeat(32),
                boot_id: format!("windows:{}:1", "b".repeat(32)),
                package_sha256: "c".repeat(64),
            },
            run_id: "run".into(),
            owner_id: "controller".into(),
            epoch: 1,
        };
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
}
