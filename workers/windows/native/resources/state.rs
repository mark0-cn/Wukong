//! Owned native resources use the existing controller fence,
//! SQLite effects and suspended Job launcher, not a second worker scheduler.
use crate::executors::parameters::{clean, request_id};
use crate::executors::specification::Specification;
use crate::executors::{analysis, shell};
use crate::executors::{
    launcher::{Launched, Running},
    plan::{Plan, Prepared},
};
use crate::resources::models::{Phase, ResourceKind, ResourceRecord};
use crate::{
    artifacts::inputs::Owner,
    executors::resource_process::Request,
    isolation::security::AclPolicy,
    resources::journal::{Cleanup, Journal, Outcome, Reservation},
};
use crate::{
    executors::session_wire::{self, request::Operation},
    resources::models::MutationKind,
};
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};

fn job_kind(record: &ResourceRecord) -> Result<&'static str> {
    let replay: Option<crate::executors::resource_replay::Configuration> =
        serde_json::from_value(record.specification["replay"].clone())?;
    Ok(replay.map_or("process", |replay| replay.kind.name()))
}

struct Starting {
    resource: String,
    request_id: String,
    ticket: crate::resources::journal::Ticket,
    task: tokio::task::JoinHandle<Result<Launched>>,
}

pub(crate) struct ProcessStart {
    owner: Owner,
    resource: String,
    request_id: String,
    kind: ResourceKind,
    pub(crate) response: crate::executors::plan::Response,
}
impl ProcessStart {
    pub(crate) fn complete(self, status: Value) -> Result<Dispatch> {
        self.response.dispatch(&self.owner, status)
    }
}
impl crate::executors::plan::Response {
    fn dispatch(self, owner: &Owner, status: Value) -> Result<Dispatch> {
        if matches!(self, Self::Session) {
            return Ok(Dispatch::Ready(status));
        }
        let resource = status["job_id"]
            .as_str()
            .context("started resource ID missing")?
            .to_owned();
        Ok(match self {
            Self::Job => Dispatch::Ready(status),
            Self::Session => unreachable!(),
            Self::Command(command) => Dispatch::Command(Box::new(shell::Pending {
                owner: owner.clone(),
                resource,
                command,
            })),
            Self::Analysis => Dispatch::Analysis(Box::new(analysis::Pending {
                owner: owner.clone(),
                resource,
            })),
        })
    }
}
struct Recovering {
    request: crate::resources::resource_recovery::Request,
    observation: super::observation::Observation<Vec<u8>>,
}
struct Finishing {
    request: Request,
    observation: super::observation::Observation<()>,
}
#[path = "session.rs"]
mod session;
pub(crate) enum Dispatch {
    Ready(Value),
    Admission(Box<Plan>),
    Starting(Box<ProcessStart>),
    Session(Box<session::Pending>),
    Command(Box<shell::Pending>),
    Analysis(Box<analysis::Pending>),
}
/// Capacity is one: mutually exclusive live/closing/recovery states own the OS controls.
enum ActiveResource {
    Idle,
    Starting(Box<Starting>),
    Process(Box<Running>),
    Finishing(Box<Finishing>),
    Session(Box<crate::executors::session_runtime::Live>),
    ClosingSession(Box<crate::executors::session_runtime::Live>),
    Recovering(Box<Recovering>),
}
impl ActiveResource {
    fn process(&self) -> Option<&Running> {
        match self {
            Self::Process(value) => Some(value),
            _ => None,
        }
    }
    fn session(&self) -> Option<&crate::executors::session_runtime::Live> {
        match self {
            Self::Session(value) | Self::ClosingSession(value) => Some(value),
            _ => None,
        }
    }
    fn session_mut(&mut self) -> Option<&mut crate::executors::session_runtime::Live> {
        match self {
            Self::Session(value) | Self::ClosingSession(value) => Some(value),
            _ => None,
        }
    }
    fn recovery(&self) -> Option<&Recovering> {
        match self {
            Self::Recovering(value) => Some(value),
            _ => None,
        }
    }
    fn closing(&mut self) {
        let previous = std::mem::replace(self, Self::Idle);
        *self = match previous {
            Self::Session(live) => Self::ClosingSession(live),
            value => value,
        };
    }
}
pub(crate) struct ResourceState {
    journal: Journal,
    context: Option<crate::host::context::RuntimeContext>,
    active: ActiveResource,
}
impl ResourceState {
    #[cfg(test)]
    pub(crate) fn test_journal(&mut self) -> &mut Journal {
        &mut self.journal
    }

    pub(crate) fn idle(&self) -> Result<bool> {
        self.journal.idle()
    }
    pub(crate) fn new(journal: Journal) -> Self {
        Self {
            journal,
            context: None,
            active: ActiveResource::Idle,
        }
    }
    pub(crate) fn initialize(
        &mut self,
        context: crate::host::context::RuntimeContext,
    ) -> Result<()> {
        ensure!(
            self.context.is_none(),
            "resource manager is already initialized"
        );
        self.context = Some(context);
        Ok(())
    }
    pub(crate) fn initialized(&self) -> bool {
        self.context.is_some()
    }
    fn host(&self) -> Result<&crate::host::context::RuntimeContext> {
        self.context
            .as_ref()
            .context("resource host is not initialized")
    }
    pub(crate) fn handles(method: &str, params: &Map<String, Value>) -> bool {
        matches!(
            crate::rpc::catalog::route(method),
            Some(crate::rpc::catalog::Route::Execution | crate::rpc::catalog::Route::Analysis)
        ) || matches!(method, "file.get" | "file.stat" | "file.read")
            && params
                .get("path")
                .and_then(Value::as_str)
                .is_some_and(|path| path.starts_with("native-results/"))
    }
    pub(crate) fn prepare_rpc(
        &mut self,
        owner: &Owner,
        method: &str,
        params: &Map<String, Value>,
    ) -> Result<Dispatch> {
        if let Some(crate::rpc::catalog::Handler::Launch(factory)) =
            crate::rpc::catalog::handler(wukong_worker_protocol::Method::parse(method)?)
        {
            let _ = self.poll();
            return self.start_plan(owner, factory(params)?);
        }
        if matches!(
            crate::rpc::catalog::handler(wukong_worker_protocol::Method::parse(method)?),
            Some(crate::rpc::catalog::Handler::Session)
        ) {
            return self.prepare_session(owner, method, params);
        }
        self.call(owner, method, params).map(Dispatch::Ready)
    }
    pub(crate) fn call(
        &mut self,
        owner: &Owner,
        method: &str,
        params: &Map<String, Value>,
    ) -> Result<Value> {
        let _ = self.poll();
        if matches!(method, "job.list" | "resource.list") {
            return self.list(owner, method, params);
        }
        if method.starts_with("file.") {
            return self.read_artifact(owner, method, params);
        }
        let id = params
            .get("job_id")
            .and_then(Value::as_str)
            .context("native job_id is required")?;
        let record = self.journal.resource(owner, id)?;
        ensure!(
            record.kind == ResourceKind::Process,
            "native resource is not a process job"
        );
        if method == "job.cancel" && record.phase != Phase::Closed {
            self.cancel(id)?;
        }
        if method == "job.artifacts" {
            if record.phase != Phase::Closed {
                return Ok(json!([]));
            }
            let status = self.status(owner, id)?;
            if status["result"]["outcome"] == "unknown" {
                let name = status["result"]["recovery_artifact"]
                    .as_str()
                    .context("native recovery artifact is absent")?;
                return Ok(json!([self.host()?.artifacts.artifact(id, name)?]));
            }
            if record.specification["replay"].is_object() {
                if job_kind(&record)? == "minimize" {
                    let mut artifacts = vec![self.host()?.artifacts.artifact(id, "receipt")?];
                    if !status["result"]["artifact"].is_null() {
                        artifacts.push(self.host()?.artifacts.artifact(id, "minimized")?);
                    }
                    return Ok(Value::Array(artifacts));
                }
                let count = status["result"]["trials"]
                    .as_u64()
                    .context("replay trial count missing")?;
                ensure!(count <= 16, "replay artifact count exceeds its bound");
                let mut artifacts = vec![self.host()?.artifacts.artifact(id, "receipt")?];
                for index in 0..count {
                    artifacts.push(
                        self.host()?
                            .artifacts
                            .artifact(id, &format!("trial-{index}"))?,
                    );
                }
                return Ok(Value::Array(artifacts));
            }
            return Ok(json!([
                self.host()?.artifacts.artifact(id, "stdout")?,
                self.host()?.artifacts.artifact(id, "stderr")?,
                self.host()?.artifacts.artifact(id, "receipt")?
            ]));
        }
        self.status(owner, id)
    }
    fn existing_start(
        &self,
        owner: &Owner,
        request_id: &str,
        kind: ResourceKind,
        specification: &Value,
        response: crate::executors::plan::Response,
    ) -> Result<Option<Dispatch>> {
        if let Some(existing) = self
            .journal
            .lookup_start(owner, request_id, kind, specification)?
        {
            let value = if kind == ResourceKind::Process {
                self.reserved_status(owner, existing)?
            } else {
                session::cached(existing)?
            };
            return response.dispatch(owner, value).map(Some);
        }
        Ok(None)
    }
    fn start_plan(&mut self, owner: &Owner, plan: Plan) -> Result<Dispatch> {
        if let Some(cached) = self.existing_start(
            owner,
            &plan.request_id,
            plan.kind,
            &plan.specification,
            plan.response.clone(),
        )? {
            return Ok(cached);
        }
        if !matches!(self.active, ActiveResource::Idle) {
            return Err(wukong_worker_protocol::CapacityBusy.into());
        }
        Ok(Dispatch::Admission(Box::new(plan)))
    }
    pub(crate) fn reserve_prepared(
        &mut self,
        owner: &Owner,
        mut prepared: Prepared,
    ) -> Result<Dispatch> {
        // Reconcile again after lock-free input preparation; another request may have reserved capacity.
        if let Some(cached) = self.existing_start(
            owner,
            &prepared.request_id,
            prepared.kind,
            &prepared.specification,
            prepared.response.clone(),
        )? {
            return Ok(cached);
        }
        if !matches!(self.active, ActiveResource::Idle) {
            return Err(wukong_worker_protocol::CapacityBusy.into());
        }
        let reserved = self.journal.reserve_start(
            owner,
            &prepared.request_id,
            prepared.kind,
            &prepared.specification,
        )?;
        let Reservation::Reserved {
            effect, resource, ..
        } = reserved
        else {
            anyhow::bail!("resource reservation changed while locked")
        };
        prepared.target.bind(resource.clone());
        let ticket = self.journal.begin(&effect)?;
        let context = self.host()?.clone();
        let pending = ProcessStart {
            owner: owner.clone(),
            resource: resource.clone(),
            request_id: prepared.request_id.clone(),
            kind: prepared.kind,
            response: prepared.response,
        };
        self.active = ActiveResource::Starting(Box::new(Starting {
            ticket,
            resource,
            request_id: prepared.request_id,
            task: tokio::spawn(crate::executors::launcher::launch(context, prepared.target)),
        }));
        Ok(Dispatch::Starting(Box::new(pending)))
    }
    fn poll_startup(&mut self) -> Result<()> {
        use futures_util::FutureExt;
        let ActiveResource::Starting(starting) = &mut self.active else {
            return Ok(());
        };
        let Some(result) = (&mut starting.task).now_or_never() else {
            return Ok(());
        };
        let ActiveResource::Starting(starting) =
            std::mem::replace(&mut self.active, ActiveResource::Idle)
        else {
            unreachable!()
        };
        let launched = result
            .context("native host launcher interrupted")
            .and_then(|result| result)?;
        match launched {
            Launched::Process(running) => {
                let value = json!({"job_id":starting.resource,"kind":running.request.kind(),"state":"running","call_id":starting.request_id,"native_host_pid":running.host.pid(),"isolation":"appcontainer"});
                self.active = ActiveResource::Process(running);
                self.journal
                    .complete(&starting.ticket, &Outcome::Result { value })
            }
            Launched::Session(live, exchange) => {
                self.active = ActiveResource::Session(live);
                self.complete_session(session::Completed {
                    resource: starting.resource,
                    ticket: Some(starting.ticket),
                    close: false,
                    exchange: *exchange,
                })
                .map(|_| ())
            }
        }
    }
    pub(crate) fn poll_started(&mut self, pending: &ProcessStart) -> Result<Option<Value>> {
        let _ = self.poll();
        let record = self.journal.resource(&pending.owner, &pending.resource)?;
        let reservation = self
            .journal
            .lookup_start(
                &pending.owner,
                &pending.request_id,
                pending.kind,
                &record.specification,
            )?
            .context("startup receipt is missing")?;
        match reservation {
            Reservation::Completed {
                outcome: Outcome::Result { value },
                ..
            } => Ok(Some(value)),
            Reservation::Pending { .. } if matches!(&self.active, ActiveResource::Starting(starting) if starting.resource == pending.resource) => {
                Ok(None)
            }
            _ => anyhow::bail!("native launch outcome is unknown; reconcile its original resource"),
        }
    }
    fn list(&self, owner: &Owner, method: &str, params: &Map<String, Value>) -> Result<Value> {
        let mut params = params.clone();
        for name in ["_binding", "_lease", "request_id", "call_id"] {
            params.remove(name);
        }
        let listing: wukong_worker_protocol::params::ResourceList =
            serde_json::from_value(Value::Object(params))?;
        let limit = usize::try_from(listing.limit.unwrap_or(64))?;
        let records = self.journal.resources(
            owner,
            if method == "job.list" {
                Some(ResourceKind::Process)
            } else {
                None
            },
            listing.after.as_deref(),
            limit,
        )?;
        if method == "job.list" {
            return records
                .iter()
                .map(|record| self.status(owner, &record.id))
                .collect::<Result<Vec<_>>>()
                .map(Value::Array);
        }
        let next_after = if records.len() == limit {
            records.last().map(|record| record.id.clone())
        } else {
            None
        };
        let resources = records.into_iter().map(|record| {
            Ok(json!({"resource_id":record.id,"kind":record.kind,"phase":record.phase,"native_owner":record.owner,"start_effect":record.start_effect,"specification_sha256":crate::artifacts::inputs::digest(&record.specification)?}))
        }).collect::<Result<Vec<_>>>()?;
        Ok(
            json!({"schema":1,"resources":resources,"next_after":next_after,"isolation":"appcontainer"}),
        )
    }
    fn reserved_status(&self, owner: &Owner, reserved: Reservation) -> Result<Value> {
        match reserved {
            Reservation::Completed {
                outcome: Outcome::Result { value },
                ..
            } => Ok(value),
            Reservation::Completed { .. } => {
                anyhow::bail!("native start outcome is unknown after cleanup")
            }
            Reservation::Pending { resource, .. } | Reservation::Reserved { resource, .. } => {
                self.status(owner, &resource)
            }
        }
    }
    fn status(&self, owner: &Owner, id: &str) -> Result<Value> {
        let record = self.journal.resource(owner, id)?;
        if record.phase == Phase::Closed {
            let path = self.host()?.artifacts.result_path(id, "status.json")?;
            AclPolicy::private_vm().verify(&path, false)?;
            let status: Value =
                serde_json::from_slice(&crate::artifacts::io::read_bytes(&path, 65536)?)?;
            ensure!(
                status["job_id"] == id
                    && status["native_owner"] == serde_json::to_value(record.owner)?,
                "native terminal status binding differs"
            );
            return Ok(status);
        }
        if matches!(&self.active, ActiveResource::Starting(starting) if starting.resource == id) {
            return Ok(
                json!({"job_id":id,"state":"queued","cleanup_pending":false,"isolation":"appcontainer"}),
            );
        }
        if let ActiveResource::Finishing(finishing) = &self.active
            && finishing.request.resource_id == id
        {
            return Ok(
                json!({"job_id":id,"kind":finishing.request.kind(),"state":"running","cleanup_pending":true,"isolation":"appcontainer"}),
            );
        }
        if let Some(running) = self
            .active
            .process()
            .filter(|running| running.request.resource_id == id)
        {
            return Ok(
                json!({"job_id":id,"kind":running.request.kind(),"state":if running.failed.is_some() {"lost"} else {"running"},"native_host_pid":running.host.pid(),"error":running.failed,"cleanup_pending":running.failed.is_some(),"isolation":"appcontainer"}),
            );
        }
        Ok(
            json!({"job_id":id,"kind":job_kind(&record)?,"state":"lost","cleanup_pending":true,"error":"native host ownership was interrupted; cleanup reconciliation is required","isolation":"appcontainer"}),
        )
    }
    pub(crate) fn cancel_active(&mut self) -> Result<()> {
        if let ActiveResource::Starting(starting) = &self.active {
            return self.cancel(&starting.resource);
        }
        if let Some(id) = self
            .active
            .session()
            .map(|debugger| debugger.request.target.resource_id.clone())
        {
            self.cancel(&id)?;
            self.active.session_mut().unwrap().request_cancel();
        }
        if let Some(running) = self.active.process() {
            self.cancel(&running.request.resource_id)?;
        }
        Ok(())
    }
    fn cancel(&self, id: &str) -> Result<()> {
        let path = self.host()?.artifacts.request_path(id, "cancel")?;
        let private = AclPolicy::private_vm();
        if path.exists() {
            private.verify(&path, false)?;
        } else {
            crate::artifacts::io::publish_json(&path, &json!({"resource_id":id}), &private)?;
        }
        Ok(())
    }
    pub(crate) fn poll(&mut self) -> Result<()> {
        if matches!(self.active, ActiveResource::Starting(_)) {
            return self.poll_startup();
        }
        if self.active.recovery().is_some() {
            return self.poll_recovery();
        }
        if let ActiveResource::Finishing(finishing) = &mut self.active {
            match finishing.observation.poll() {
                Ok(None) => return Ok(()),
                Err(_) => return self.recover(),
                Ok(Some(_)) => {}
            }
            let request = finishing.request.clone();
            if self.finish_result(&request).is_err() {
                return self.recover();
            }
            self.active = ActiveResource::Idle;
            return Ok(());
        }
        if let Some(debugger) = self.active.session() {
            return if debugger.needs_recovery() {
                self.recover()
            } else {
                Ok(())
            };
        }
        let Some(running) = self.active.process() else {
            return self.recover();
        };
        if running.failed.is_some() {
            return self.recover();
        }
        if !running.host.exited() && Instant::now() < running.deadline {
            return Ok(());
        }
        let ActiveResource::Process(running) =
            std::mem::replace(&mut self.active, ActiveResource::Idle)
        else {
            unreachable!()
        };
        let request = running.request.clone();
        self.active = ActiveResource::Finishing(Box::new(Finishing {
            request,
            observation: super::observation::Observation::Running(tokio::task::spawn_blocking(
                move || {
                    ensure!(
                        running.host.stop(Duration::from_secs(10))? == 0,
                        "native host failed or exceeded preparation deadline; cleanup remains unconfirmed"
                    );
                    Ok(())
                },
            )),
        }));
        Ok(())
    }
    fn read_artifact(
        &self,
        owner: &Owner,
        method: &str,
        params: &Map<String, Value>,
    ) -> Result<Value> {
        let path = params["path"]
            .as_str()
            .context("artifact path is missing")?;
        let parts: Vec<_> = path.split('/').collect();
        ensure!(
            parts.len() == 3 && parts[0] == "native-results",
            "invalid native artifact selector"
        );
        let record = self.journal.resource(owner, parts[1])?;
        self.host()?.artifacts.read(&record, method, params)
    }
    fn finish_result(&mut self, request: &Request) -> Result<()> {
        if request.replay.is_some() {
            return self.finish_replay_result(request);
        }
        let id = &request.resource_id;
        let private = AclPolicy::private_vm();
        let path = self.host()?.artifacts.result_path(id, "json")?;
        private.verify(&path, false)?;
        let bytes = crate::artifacts::io::read_bytes(&path, 12 * 1024 * 1024)?;
        let result: Value = serde_json::from_slice(&bytes)?;
        let streams = crate::executors::resource_process::output(request, &result)?;
        for (name, data) in ["stdout", "stderr"].into_iter().zip(&streams) {
            self.host()?
                .artifacts
                .retain(&self.host()?.artifacts.result_path(id, name)?, data)?;
        }
        let reason = result["stop_reason"]
            .as_str()
            .context("native stop reason is absent")?;
        let exit = result["exit_code"]
            .as_u64()
            .context("native exit is absent")?;
        let state = match reason {
            "exited" => {
                if exit == 0 {
                    "completed"
                } else {
                    "failed"
                }
            }
            "cancelled" => "cancelled",
            "deadline" => "timed_out",
            "output_limit" | "io_failure" => "failed",
            _ => anyhow::bail!("invalid native stop reason"),
        };
        fn tail(bytes: &[u8]) -> String {
            String::from_utf8_lossy(&bytes[bytes.len().saturating_sub(4096)..]).into()
        }
        let value = json!({"job_id":id,"kind":"process","state":state,"native_owner":request.owner,"stdout":tail(&streams[0]),"stderr":tail(&streams[1]),"result":{"exit_code":exit,"stop_reason":reason,"stdout_sha256":result["stdout_sha256"],"stderr_sha256":result["stderr_sha256"],"stdin_written":result["stdin_written"],"stdin_complete":result["stdin_complete"],"stdin_error":result["stdin_error"],"native_receipt_sha256":hex::encode(Sha256::digest(&bytes)),"jobs_empty":true,"profiles_removed":true,"scratch_removed":true,
            "duration_secs":result["duration_secs"],"output_bytes_retained":{"stdout":streams[0].len(),"stderr":streams[1].len()},"output_truncated":reason == "output_limit" || streams.iter().any(|stream|stream.len()>4096)},"isolation":"appcontainer"});
        self.host()?.artifacts.retain(
            &self.host()?.artifacts.result_path(id, "status.json")?,
            &serde_json::to_vec(&value)?,
        )?;
        self.journal.start_closing(id)?;
        self.journal.close(
            id,
            Cleanup {
                jobs_empty: true,
                profiles_removed: true,
                scratch_removed: true,
            },
        )?;
        Ok(())
    }

    pub(crate) fn poll_command(&mut self, pending: &shell::Pending) -> Result<Option<Value>> {
        let _ = self.poll();
        let record = self.journal.resource(&pending.owner, &pending.resource)?;
        ensure!(
            record.kind == ResourceKind::Process
                && record.specification["native_shell_schema"] == 1,
            "native command observation does not belong to its shell resource"
        );
        if record.phase != Phase::Closed {
            return Ok(None);
        }
        let status = self.status(&pending.owner, &pending.resource)?;
        let result = &status["result"];
        let unknown = status["state"] == "lost";
        let artifact = if unknown {
            result["recovery_artifact"]
                .as_str()
                .context("native shell recovery receipt absent")?
        } else {
            "receipt"
        };
        Ok(Some(
            json!({"state":status["state"],"command":pending.command,"engine":"cmd",
            "resource_id":pending.resource,"native_owner":status["native_owner"],
            "exit_code":result["exit_code"],"signal":null,"duration_secs":result["duration_secs"],
            "stdout":status["stdout"],"stderr":status["stderr"],
            "output_bytes_retained":result["output_bytes_retained"],"output_truncated":result["output_truncated"],
            "receipt":format!("native-results/{}/{artifact}",pending.resource),
            "stdout_artifact":if unknown {None}else{Some(format!("native-results/{}/stdout",pending.resource))},
            "stderr_artifact":if unknown {None}else{Some(format!("native-results/{}/stderr",pending.resource))},
            "cleanup_pending":false,"result":result,"isolation":"appcontainer"}),
        ))
    }

    pub(crate) fn poll_analysis(&mut self, pending: &analysis::Pending) -> Result<Option<Value>> {
        let _ = self.poll();
        let record = self.journal.resource(&pending.owner, &pending.resource)?;
        ensure!(
            record.specification["native_analysis_schema"] == 1,
            "resource is not an analysis invocation"
        );
        if record.phase != Phase::Closed {
            return Ok(None);
        }
        let status = self.status(&pending.owner, &pending.resource)?;
        ensure!(
            status["state"] != "lost",
            "analysis outcome is unknown; reconcile the original resource"
        );
        let output = self.read_artifact(
            &pending.owner,
            "file.get",
            json!({"path":format!("native-results/{}/stdout",pending.resource)})
                .as_object()
                .unwrap(),
        )?;
        let bytes = STANDARD.decode(
            output["data_base64"]
                .as_str()
                .context("analysis output missing")?,
        )?;
        let reply: Value =
            serde_json::from_slice(&bytes).context("analysis tool returned invalid JSON")?;
        ensure!(
            status["result"]["exit_code"] == 0 && reply.get("error").is_none(),
            "analysis failed: {}",
            reply["error"].as_str().unwrap_or("resource terminated")
        );
        let mut result = reply
            .get("result")
            .cloned()
            .filter(Value::is_object)
            .context("analysis result must be an object")?;
        result["execution_evidence"] = json!({"resource_id":pending.resource,"native_owner":record.owner,
            "stdout":output,"receipt":self.host()?.artifacts.artifact(&pending.resource,"receipt")?});
        result["execution_evidence"]["stdout"]
            .as_object_mut()
            .unwrap()
            .remove("data_base64");
        Ok(Some(result))
    }

    pub(super) fn finish_replay_result(&mut self, request: &Request) -> Result<()> {
        let id = &request.resource_id;
        let path = self.host()?.artifacts.result_path(id, "json")?;
        AclPolicy::private_vm().verify(&path, false)?;
        let minimizing = request
            .replay
            .as_ref()
            .is_some_and(|replay| replay.configuration.minimization.is_some());
        let bytes = crate::artifacts::io::read_bytes(
            &path,
            if minimizing { 4 * 1024 * 1024 } else { 65536 },
        )?;
        let receipt: Value = serde_json::from_slice(&bytes)?;
        let mut result = if minimizing {
            crate::executors::resource_minimize::summarize(request, &receipt, |suffix| {
                let path = self.host()?.artifacts.result_path(id, suffix)?;
                AclPolicy::private_vm().verify(&path, false)?;
                crate::artifacts::io::read_bytes(&path, 32 * 1024 * 1024)
            })?
        } else {
            crate::executors::resource_replay::summarize(request, &receipt, |index| {
                let path = self
                    .host()?
                    .artifacts
                    .result_path(id, &format!("trial-{index}.json"))?;
                AclPolicy::private_vm().verify(&path, false)?;
                crate::artifacts::io::read_bytes(&path, 12 * 1024 * 1024)
            })?
        };
        result["native_receipt_sha256"] = json!(hex::encode(Sha256::digest(&bytes)));
        let status = json!({"job_id":id,"kind":request.kind(),"state":if receipt["cancelled"] == true {"cancelled"} else if minimizing && !receipt["failed"].is_null() {"failed"} else {"completed"},
            "native_owner":request.owner,"result":result,"isolation":"appcontainer"});
        self.host()?.artifacts.retain(
            &self.host()?.artifacts.result_path(id, "status.json")?,
            &serde_json::to_vec(&status)?,
        )?;
        self.journal.start_closing(id)?;
        self.journal.close(
            id,
            Cleanup {
                jobs_empty: true,
                profiles_removed: true,
                scratch_removed: true,
            },
        )
    }

    pub(super) fn prepare_session(
        &mut self,
        owner: &Owner,
        method: &str,
        params: &Map<String, Value>,
    ) -> Result<Dispatch> {
        let _ = self.poll();
        let interactive = method.starts_with("interact.");
        let close = matches!(method, "debug.close" | "interact.close");
        let observation = matches!(
            method,
            "debug.inspect" | "debug.profile" | "interact.profile"
        );
        if !observation {
            request_id(params)?;
        }
        let mut fields = clean(params);
        let id = fields
            .remove("session_id")
            .and_then(|v| v.as_str().map(str::to_owned))
            .context("native session session_id missing")?;
        let record = self.journal.resource(owner, &id)?;
        ensure!(
            record.kind
                == if interactive {
                    ResourceKind::Interactive
                } else {
                    ResourceKind::Debugger
                },
            "native resource session kind differs"
        );
        if interactive && close && record.phase == Phase::Closed {
            ensure!(
                fields.is_empty(),
                "interactive close accepts only session_id"
            );
            return self
                .host()?
                .artifacts
                .session_terminal(&record, &AclPolicy::private_vm())
                .map(Dispatch::Ready);
        }
        let operation = match method {
            "debug.profile" | "interact.profile" => {
                ensure!(fields.is_empty(), "session profile accepts only session_id");
                Operation::Profile(session_wire::Profile {})
            }
            "debug.inspect" => {
                let query: session_wire::Inspect =
                    serde_json::from_value(Value::Object(fields.clone()))?;
                let kind = query.validate()?;
                if kind == session_wire::Inspection::State && record.phase == Phase::Closed {
                    return self
                        .host()?
                        .artifacts
                        .session_terminal(&record, &AclPolicy::private_vm())
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
            "interact.write" => {
                let write: session::InteractiveWrite =
                    serde_json::from_value(Value::Object(fields))?;
                let write = write.decode()?;
                // Effects retain a content identity, not megabytes of base64.
                fields = json!({"bytes":write.data.len(),"sha256":hex::encode(Sha256::digest(&write.data)),"read_timeout_secs":write.read_timeout_seconds}).as_object().unwrap().clone();
                Operation::Write(write)
            }
            "debug.close" | "interact.close" => {
                ensure!(
                    fields.is_empty(),
                    "native session close accepts only session_id"
                );
                Operation::Close(session_wire::Close {})
            }
            _ => anyhow::bail!("unsupported native session method"),
        };
        if close
            && matches!(&self.active, ActiveResource::Starting(starting) if starting.resource == id)
        {
            self.cancel(&id)?;
            anyhow::bail!(
                "native session startup cancellation requested; owned cleanup is still pending"
            );
        }
        if close
            && self
                .active
                .session()
                .is_some_and(|live| live.request.target.resource_id == id && live.busy())
        {
            self.cancel(&id)?;
            self.active.session_mut().unwrap().request_cancel();
            self.journal.start_closing(&id)?;
            self.active.closing();
            anyhow::bail!("native session cancellation requested; owned cleanup is still pending");
        }
        ensure!(
            !self
                .active
                .session()
                .is_some_and(|live| live.request.target.resource_id == id && live.busy()),
            "native session already has an in-flight exchange"
        );
        let ticket = if observation {
            None
        } else {
            let reservation = self.journal.reserve_mutation(
                owner,
                request_id(params)?,
                &id,
                if interactive {
                    MutationKind::InteractiveWrite
                } else {
                    MutationKind::DebugControl
                },
                &json!({"method":method,"params":fields}),
            )?;
            let Reservation::Reserved { effect, .. } = reservation else {
                return session::cached(reservation).map(Dispatch::Ready);
            };
            Some(self.journal.begin(&effect)?)
        };
        ensure!(
            record.phase == Phase::Live,
            "native session is not live; reconcile its original resource"
        );
        let live = self
            .active
            .session_mut()
            .filter(|live| live.request.target.resource_id == id)
            .context("native session host ownership was interrupted")?;
        let exchange = match live.prepare(operation) {
            Ok(exchange) => exchange,
            Err(error) => {
                live.failed = Some(format!("{error:#}").chars().take(2048).collect());
                return Err(error);
            }
        };
        if close {
            self.journal.start_closing(&id)?;
            self.active.closing();
        }
        Ok(Dispatch::Session(Box::new(session::Pending {
            resource: id,
            ticket,
            close,
            exchange,
        })))
    }
    /// Read only after journal-confirmed cleanup. The same run's successor can
    /// inspect original ownership; it cannot turn this projection into a start.
    pub(crate) fn complete_session(&mut self, completed: session::Completed) -> Result<Value> {
        let root = self.host()?.root.clone();
        let live = self
            .active
            .session_mut()
            .filter(|live| live.request.target.resource_id == completed.resource)
            .context("native session completion lost its original host; reconcile the journal")?;
        let mut closed = completed.close;
        let result = (|| -> Result<Value> {
            let value = live.complete(completed.exchange)?;
            let record = self
                .journal
                .resource(&live.request.target.owner, &completed.resource)?;
            ensure!(
                if completed.close {
                    record.phase == Phase::Closing
                } else {
                    matches!(record.phase, Phase::Preparing | Phase::Live)
                },
                "native session was cancelled while its operation was in flight"
            );
            ensure!(
                value["session_id"] == completed.resource
                    && value["worker_session_id"] == completed.resource
                    && value["native_owner"] == serde_json::to_value(&live.request.target.owner)?,
                "native session reply ownership differs"
            );
            let exited =
                live.request.kind() == ResourceKind::Interactive && value["state"] == "exited";
            ensure!(
                value["state"] == if completed.close { "closed" } else { "running" }
                    || !completed.close && exited,
                "native session reply lifecycle differs"
            );
            closed |= exited;
            if closed {
                live.finish()?;
                let prefix = crate::executors::session_request::prefix(live.request.kind())?;
                let path = root
                    .join("metadata/resource-results")
                    .join(format!("{}.{prefix}-result.json", completed.resource));
                AclPolicy::private_vm().verify(&path, false)?;
                let proof: Value =
                    serde_json::from_slice(&crate::artifacts::io::read_bytes(&path, 65536)?)?;
                ensure!(
                    proof == value["response"]["cleanup"]
                        && proof["schema"] == crate::executors::session_host::SCHEMA
                        && proof["scope"] == live.request.scope()
                        && proof["owner"] == serde_json::to_value(&live.request.target.owner)?
                        && proof["resource_id"] == completed.resource
                        && proof["request_sha256"] == live.request_hash
                        && proof["read_only"] == live.request.read_only()
                        && proof["target_executed"] == !live.request.read_only()
                        && proof["outcome"] == "closed"
                        && proof["jobs_empty"] == true
                        && proof["profiles_removed"] == true
                        && proof["scratch_removed"] == true,
                    "native session cleanup proof differs"
                );
            }
            if let Some(ticket) = &completed.ticket {
                self.journal.complete(
                    ticket,
                    &Outcome::Result {
                        value: value.clone(),
                    },
                )?;
            }
            if closed {
                self.journal.start_closing(&completed.resource)?;
                self.journal.close(
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
            Ok(_) if closed => {
                self.active = ActiveResource::Idle;
            }
            Err(error) => {
                live.failed = Some(format!("{error:#}").chars().take(2048).collect());
                self.cancel(&completed.resource)?;
            }
            _ => {}
        }
        result
    }

    pub(super) fn recover(&mut self) -> Result<()> {
        if self.active.recovery().is_some() {
            return Ok(());
        }
        let Some(record) = self.journal.active()? else {
            return Ok(());
        };
        ensure!(
            self.active
                .process()
                .is_none_or(|running| running.failed.is_some()),
            "cannot recover an executing native host"
        );
        ensure!(
            self.active
                .session()
                .is_none_or(|debugger| debugger.needs_recovery()),
            "cannot recover a live debugger host"
        );
        let attempt = hex::encode(crate::host::accounts::random_bytes::<16>()?);
        let request = crate::resources::resource_recovery::Request {
            schema: crate::executors::resource_process::SCHEMA,
            attempt: attempt.clone(),
            resource: record,
        };
        request.validate()?;
        let context = self.host()?.clone();
        let previous = std::mem::replace(&mut self.active, ActiveResource::Idle);
        let observed_request = request.clone();
        self.active = ActiveResource::Recovering(Box::new(Recovering {
            request,
            observation: super::observation::Observation::Running(tokio::task::spawn_blocking(
                move || {
                    // Releasing old controls can drain a Job; never do that under the manager lock.
                    drop(previous);
                    super::recovery_observer::observe(context, observed_request)
                },
            )),
        }));
        Ok(())
    }
    pub(super) fn poll_recovery(&mut self) -> Result<()> {
        let ActiveResource::Recovering(recovery) = &mut self.active else {
            anyhow::bail!("native recovery host vanished")
        };
        let Some(bytes) = recovery.observation.poll()? else {
            return Ok(());
        };
        let bytes = bytes.clone();
        let record = recovery.request.resource.clone();
        let artifact = format!("recovery-{}", recovery.request.attempt);
        let result: Value = serde_json::from_slice(&bytes)?;
        ensure!(
            result["schema"] == crate::executors::resource_process::SCHEMA
                && result["scope"] == "owned_resource_recovery"
                && result["attempt"] == recovery.request.attempt
                && result["resource_id"] == record.id
                && result["owner"] == serde_json::to_value(&record.owner)?
                && result["record_sha256"] == crate::artifacts::inputs::digest(&record)?
                && result["jobs_empty"] == true
                && result["profiles_removed"] == true
                && result["scratch_removed"] == true,
            "native recovery proof binding or cleanup differs"
        );
        if record.kind == ResourceKind::Process
            && self
                .host()?
                .artifacts
                .result_path(&record.id, "json")?
                .try_exists()?
        {
            let path = self.host()?.artifacts.request_path(&record.id, "json")?;
            AclPolicy::private_vm().verify(&path, false)?;
            let request: Request =
                serde_json::from_slice(&crate::artifacts::io::read_bytes(&path, 65536)?)?;
            request.validate()?;
            let spec: Specification =
                serde_json::from_value(record.specification["process"].clone())?;
            ensure!(
                spec.matches(&request)
                    && serde_json::from_value::<
                        Option<crate::executors::resource_process::Interpreter>,
                    >(record.specification["interpreter"].clone())?
                        == request.interpreter
                    && serde_json::from_value::<
                        Option<crate::executors::resource_replay::Configuration>,
                    >(record.specification["replay"].clone())?
                        == request
                            .replay
                            .as_ref()
                            .map(|replay| replay.configuration.clone())
                    && request.owner == record.owner
                    && request.resource_id == record.id,
                "recovered result request differs from its journal intent"
            );
            self.finish_result(&request)?;
        } else {
            self.finish_unknown(&record, &artifact, &bytes)?;
        }
        self.active = ActiveResource::Idle;
        Ok(())
    }
    fn finish_unknown(
        &mut self,
        record: &ResourceRecord,
        artifact: &str,
        receipt: &[u8],
    ) -> Result<()> {
        let path = self
            .host()?
            .artifacts
            .result_path(&record.id, "status.json")?;
        if path.try_exists()? {
            AclPolicy::private_vm().verify(&path, false)?;
            let prior: Value =
                serde_json::from_slice(&crate::artifacts::io::read_bytes(&path, 65536)?)?;
            ensure!(
                prior[if record.kind != ResourceKind::Process {
                    "session_id"
                } else {
                    "job_id"
                }] == record.id
                    && prior["native_owner"] == serde_json::to_value(&record.owner)?
                    && prior["state"] == "lost"
                    && prior["result"]["outcome"] == "unknown",
                "previous interrupted status differs"
            );
        } else {
            let mut value = json!({"job_id":record.id,"kind":record.kind,"state":"lost","native_owner":record.owner,"cleanup_pending":false,"result":{"outcome":"unknown","jobs_empty":true,"profiles_removed":true,"scratch_removed":true,"recovery_artifact":artifact,"recovery_receipt_sha256":hex::encode(Sha256::digest(receipt))},"isolation":"appcontainer"});
            if record.kind != ResourceKind::Process {
                value.as_object_mut().unwrap().remove("job_id");
                value["session_id"] = json!(record.id);
                value["worker_session_id"] = json!(record.id);
            } else {
                value["kind"] = json!(job_kind(record)?);
            }
            self.host()?
                .artifacts
                .retain(&path, &serde_json::to_vec(&value)?)?;
        }
        self.journal.start_closing(&record.id)?;
        self.journal.close(
            &record.id,
            Cleanup {
                jobs_empty: true,
                profiles_removed: true,
                scratch_removed: true,
            },
        )
    }
}

#[cfg(test)]
#[path = "state_tests.rs"]
pub(crate) mod tests;
