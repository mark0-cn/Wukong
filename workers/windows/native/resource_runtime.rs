//! Owned native resources use the existing controller fence,
//! SQLite effects and suspended Job launcher, not a second worker scheduler.
use crate::{
    deployment::{self, Deployment},
    inputs::{Inputs, Owner},
    processes::{Spawn, SystemHost},
    resource_input::Purpose,
    resource_journal::{
        Cleanup, Journal, Outcome, Phase, Reservation, ResourceKind, ResourceRecord,
    },
    resource_process::Request,
    security::{self, AclPolicy},
};
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Specification {
    #[serde(default)]
    argv: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    launch_spec: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    input: Option<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default = "default_cwd")]
    cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timeout_secs: Option<u64>,
    #[serde(default = "default_output")]
    output_limit: usize,
}

fn default_cwd() -> String {
    ".".into()
}
fn default_timeout() -> u64 {
    30
}
fn default_output() -> usize {
    1024 * 1024
}
fn request_id(params: &Map<String, Value>) -> Result<&str> {
    let request_id = params
        .get("request_id")
        .and_then(Value::as_str)
        .context("native request requires a stable request_id")?;
    ensure!(
        params
            .get("call_id")
            .is_none_or(|call| call.as_str() == Some(request_id)),
        "native call_id and request_id differ"
    );
    Ok(request_id)
}
fn clean(params: &Map<String, Value>) -> Map<String, Value> {
    let mut parameters = params.clone();
    for name in ["request_id", "call_id", "_binding", "_lease"] {
        parameters.remove(name);
    }
    parameters.retain(|_, value| !value.is_null());
    parameters
}
fn start_parameters<T: serde::de::DeserializeOwned>(
    params: &Map<String, Value>,
) -> Result<(&str, T)> {
    Ok((
        request_id(params)?,
        serde_json::from_value(Value::Object(clean(params)))?,
    ))
}
fn job_kind(record: &ResourceRecord) -> Result<&'static str> {
    let replay: Option<crate::resource_replay::Configuration> =
        serde_json::from_value(record.specification["replay"].clone())?;
    Ok(replay.map_or("process", |replay| replay.kind.name()))
}

impl Specification {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.cwd == "."
                && (self.launch_spec.is_some() || !self.argv.is_empty())
                && self.argv.len() <= 129
                && self.argv.iter().all(|a| !a.contains('\0'))
                && self
                    .argv
                    .iter()
                    .map(|a| a.encode_utf16().count())
                    .sum::<usize>()
                    <= 16384,
            "native process requires argv and private cwd '.'"
        );
        Ok(())
    }
    fn matches(&self, request: &Request) -> bool {
        let launch = if let Some(launch) = &request.launch {
            self.launch_spec.as_ref() == Some(&launch.manifest.path)
                && self.argv.is_empty()
                && self.env.is_empty()
                && self
                    .timeout_secs
                    .is_none_or(|timeout| timeout == request.timeout_secs)
        } else {
            self.launch_spec.is_none()
                && self.argv.first() == Some(&request.program.path)
                && request.arguments == self.argv[1..]
                && request.environment == self.env
                && request.timeout_secs == self.timeout_secs.unwrap_or(default_timeout())
        };
        launch
            && request.input.as_ref().map(|input| input.path.as_str()) == self.input.as_deref()
            && request.output_limit == self.output_limit
    }
    fn prepare(&self, inputs: &Inputs, owner: &Owner) -> Result<Request> {
        self.prepare_interpreted(inputs, owner, None)
    }
    fn prepare_interpreted(
        &self,
        inputs: &Inputs,
        owner: &Owner,
        interpreter: Option<crate::resource_process::Interpreter>,
    ) -> Result<Request> {
        self.validate()?;
        let input = self
            .input
            .as_deref()
            .map(|path| inputs.reference(owner, path, Purpose::Inspect))
            .transpose()?;
        let (program, arguments, environment, timeout, launch) =
            if let Some(path) = &self.launch_spec {
                ensure!(
                    self.argv.is_empty() && self.env.is_empty(),
                    "LaunchSpec owns argv and environment"
                );
                let (manifest, resolved) = crate::resource_launch::prepare(inputs, owner, path)?;
                ensure!(
                    self.timeout_secs
                        .is_none_or(|timeout| timeout == resolved.timeout_secs),
                    "LaunchSpec owns its execution timeout"
                );
                let binding = crate::resource_launch::Binding {
                    manifest,
                    files: resolved.files,
                    cwd: resolved.cwd,
                    input_mode: resolved.input_mode,
                };
                (
                    resolved.program,
                    resolved.arguments,
                    resolved.environment,
                    resolved.timeout_secs,
                    Some(binding),
                )
            } else {
                (
                    inputs.reference(
                        owner,
                        &self.argv[0],
                        if interpreter.is_some() {
                            Purpose::Inspect
                        } else {
                            Purpose::Execute
                        },
                    )?,
                    self.argv[1..].to_vec(),
                    self.env.clone(),
                    self.timeout_secs.unwrap_or(default_timeout()),
                    None,
                )
            };
        let mut argument_files = BTreeMap::new();
        for path in arguments.iter().filter(|_| launch.is_none()) {
            if path.starts_with("files/")
                && path != &program.path
                && !argument_files.contains_key(path)
            {
                let reference = inputs.reference(owner, path, Purpose::Inspect)?;
                argument_files.insert(path.clone(), reference);
            }
        }
        let request = Request {
            schema: crate::resource_process::SCHEMA,
            resource_id: "0".repeat(32),
            owner: owner.clone(),
            program,
            interpreter,
            replay: None,
            argument_files: argument_files.into_values().collect(),
            launch,
            arguments,
            input,
            environment,
            timeout_secs: timeout,
            output_limit: self.output_limit,
            processes: 16,
            memory_bytes: 1024 * 1024 * 1024,
        };
        request.validate()?;
        Ok(request)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Listing {
    #[serde(default)]
    after: Option<String>,
    #[serde(default = "default_list_limit")]
    limit: usize,
}
fn default_list_limit() -> usize {
    64
}

struct Running {
    request: Request,
    host: SystemHost,
    deadline: Instant,
    failed: Option<String>,
}
struct Recovering {
    request: crate::resource_recovery::Request,
    host: SystemHost,
    deadline: Instant,
}
#[path = "resource_runtime_python.rs"]
mod python;
#[path = "resource_runtime_recovery.rs"]
mod recovery;
#[path = "resource_runtime_replay.rs"]
mod replay;
#[path = "resource_runtime_session.rs"]
mod session;
#[path = "resource_runtime_shell.rs"]
mod shell;
pub(crate) enum Dispatch {
    Ready(Value),
    Session(Box<session::Pending>),
    Command(Box<shell::Pending>),
}
pub(crate) struct Runtime {
    root: PathBuf,
    executable: PathBuf,
    installation: String,
    running: Option<Running>,
    recovery: Option<Recovering>,
    session: Option<crate::session_runtime::Live>,
}
impl Runtime {
    pub(crate) fn open(
        root: &Path,
        deployment: &Deployment,
        manifest: &wukong_worker_package::VerifiedManifest,
    ) -> Result<Self> {
        security::require_system()?;
        let root = root.canonicalize()?;
        let private = AclPolicy::private_vm();
        for name in ["resource-requests", "resource-results"] {
            let directory = root.join("metadata").join(name);
            if !directory.exists() {
                private.create_directory(&directory)?;
            }
            private.verify(&directory, true)?;
        }
        Ok(Self {
            executable: deployment
                .package(&root)
                .join(&manifest.manifest().entrypoints.worker),
            root,
            installation: deployment.installation_id.clone(),
            running: None,
            recovery: None,
            session: None,
        })
    }
    pub(crate) fn handles(method: &str, params: &Map<String, Value>) -> bool {
        matches!(
            method,
            "debug.start"
                | "debug.inspect"
                | "debug.control"
                | "debug.close"
                | "interact.start"
                | "interact.write"
                | "interact.profile"
                | "interact.close"
                | "exec.run"
                | "job.start"
                | "py.run"
                | "crash.replay"
                | "input.minimize"
                | "poc.verify"
                | "job.status"
                | "job.cancel"
                | "job.artifacts"
                | "job.list"
                | "resource.list"
        ) || matches!(method, "file.get" | "file.stat" | "file.read")
            && params
                .get("path")
                .and_then(Value::as_str)
                .is_some_and(|path| path.starts_with("native-results/"))
    }
    pub(crate) fn prepare_rpc(
        &mut self,
        journal: &mut Journal,
        inputs: Option<&Inputs>,
        owner: &Owner,
        method: &str,
        params: &Map<String, Value>,
    ) -> Result<Dispatch> {
        if method.starts_with("debug.") || method.starts_with("interact.") {
            return self.prepare_session(journal, inputs, owner, method, params);
        }
        if method == "exec.run" {
            self.poll(journal)?;
            return self.start_shell(
                journal,
                inputs.context("native shell input store missing")?,
                owner,
                params,
            );
        }
        self.call(journal, inputs, owner, method, params)
            .map(Dispatch::Ready)
    }
    pub(crate) fn call(
        &mut self,
        journal: &mut Journal,
        inputs: Option<&Inputs>,
        owner: &Owner,
        method: &str,
        params: &Map<String, Value>,
    ) -> Result<Value> {
        self.poll(journal)?;
        if matches!(method, "job.list" | "resource.list") {
            return self.list(journal, owner, method, params);
        }
        if method == "job.start" {
            return self.start(
                journal,
                inputs.context("native start input store is missing")?,
                owner,
                params,
            );
        }
        if method == "py.run" {
            return self.start_python(
                journal,
                inputs.context("native Python input store is missing")?,
                owner,
                params,
            );
        }
        if matches!(method, "crash.replay" | "poc.verify" | "input.minimize") {
            return self.start_replay(
                journal,
                inputs.context("native replay input store is missing")?,
                owner,
                if method == "poc.verify" {
                    crate::resource_replay::Kind::Verify
                } else if method == "input.minimize" {
                    crate::resource_replay::Kind::Minimize
                } else {
                    crate::resource_replay::Kind::Replay
                },
                params,
            );
        }
        if method.starts_with("file.") {
            return self.read_artifact(journal, owner, method, params);
        }
        let id = params
            .get("job_id")
            .and_then(Value::as_str)
            .context("native job_id is required")?;
        let record = journal.resource(owner, id)?;
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
            let status = self.status(journal, owner, id)?;
            if status["result"]["outcome"] == "unknown" {
                let name = status["result"]["recovery_artifact"]
                    .as_str()
                    .context("native recovery artifact is absent")?;
                return Ok(json!([self.artifact(id, name)?]));
            }
            if record.specification["replay"].is_object() {
                if job_kind(&record)? == "minimize" {
                    let mut artifacts = vec![self.artifact(id, "receipt")?];
                    if !status["result"]["artifact"].is_null() {
                        artifacts.push(self.artifact(id, "minimized")?);
                    }
                    return Ok(Value::Array(artifacts));
                }
                let count = status["result"]["trials"]
                    .as_u64()
                    .context("replay trial count missing")?;
                ensure!(count <= 16, "replay artifact count exceeds its bound");
                let mut artifacts = vec![self.artifact(id, "receipt")?];
                for index in 0..count {
                    artifacts.push(self.artifact(id, &format!("trial-{index}"))?);
                }
                return Ok(Value::Array(artifacts));
            }
            return Ok(json!([
                self.artifact(id, "stdout")?,
                self.artifact(id, "stderr")?,
                self.artifact(id, "receipt")?
            ]));
        }
        self.status(journal, owner, id)
    }
    fn start(
        &mut self,
        journal: &mut Journal,
        inputs: &Inputs,
        owner: &Owner,
        params: &Map<String, Value>,
    ) -> Result<Value> {
        let (request_id, mut spec): (_, Specification) = start_parameters(params)?;
        if spec.launch_spec.is_none() && spec.timeout_secs.is_none() {
            spec.timeout_secs = Some(default_timeout());
        }
        spec.validate()?;
        let specification =
            json!({"native_process_schema":crate::resource_process::SCHEMA,"process":spec});
        self.start_prepared(journal, owner, request_id, &specification, || {
            spec.prepare(inputs, owner)
        })
    }
    fn start_prepared(
        &mut self,
        journal: &mut Journal,
        owner: &Owner,
        request_id: &str,
        specification: &Value,
        prepare: impl FnOnce() -> Result<Request>,
    ) -> Result<Value> {
        if let Some(existing) =
            journal.lookup_start(owner, request_id, ResourceKind::Process, specification)?
        {
            return self.reserved_status(journal, owner, existing);
        }
        ensure!(
            self.running.is_none() && self.session.is_none(),
            "native host remains owned"
        );
        let mut request = prepare()?;
        let reserved =
            journal.reserve_start(owner, request_id, ResourceKind::Process, specification)?;
        let Reservation::Reserved {
            effect, resource, ..
        } = reserved
        else {
            return self.reserved_status(journal, owner, reserved);
        };
        request.resource_id = resource.clone();
        let ticket = journal.begin(&effect)?;
        let private = AclPolicy::private_vm();
        let path = self.request_path(&resource, "json");
        deployment::publish_json(&path, &request, &private)?;
        let log = private.create_file(&self.request_path(&resource, "log"))?;
        let input = fs::File::open("NUL")?;
        let environment = crate::environment::system(&self.root)?;
        let host = Spawn {
            executable: &self.executable,
            arguments: &[
                "resource-execute".into(),
                self.installation.clone().into(),
                self.root.join("metadata/deployment.json").into_os_string(),
                path.into_os_string(),
            ],
            directory: &self.root,
            environment: &environment,
            log: &log,
            input: &input,
        }
        .system_host(&self.installation, &resource)?;
        let value = json!({"job_id":resource,"kind":request.kind(),"state":"running","call_id":request_id,"native_host_pid":host.pid(),"production_execution":false});
        self.running = Some(Running {
            deadline: Instant::now()
                + Duration::from_secs(
                    request
                        .replay
                        .as_ref()
                        .and_then(|replay| replay.configuration.minimization.as_ref())
                        .map_or_else(
                            || {
                                (request.timeout_secs + 120)
                                    * u64::from(
                                        request
                                            .replay
                                            .as_ref()
                                            .map_or(1, |replay| replay.configuration.trials),
                                    )
                            },
                            |minimize| minimize.timeout_secs + 120,
                        ),
                ),
            request,
            host,
            failed: None,
        });
        journal.complete(
            &ticket,
            &Outcome::Result {
                value: value.clone(),
            },
        )?;
        Ok(value)
    }
    fn list(
        &self,
        journal: &Journal,
        owner: &Owner,
        method: &str,
        params: &Map<String, Value>,
    ) -> Result<Value> {
        let mut params = params.clone();
        for name in ["_binding", "_lease", "request_id", "call_id"] {
            params.remove(name);
        }
        let listing: Listing = serde_json::from_value(Value::Object(params))?;
        let records = journal.resources(
            owner,
            if method == "job.list" {
                Some(ResourceKind::Process)
            } else {
                None
            },
            listing.after.as_deref(),
            listing.limit,
        )?;
        if method == "job.list" {
            return records
                .iter()
                .map(|record| self.status(journal, owner, &record.id))
                .collect::<Result<Vec<_>>>()
                .map(Value::Array);
        }
        let next_after = if records.len() == listing.limit {
            records.last().map(|record| record.id.clone())
        } else {
            None
        };
        let resources = records.into_iter().map(|record| {
            Ok(json!({"resource_id":record.id,"kind":record.kind,"phase":record.phase,"native_owner":record.owner,"start_effect":record.start_effect,"specification_sha256":crate::inputs::digest(&record.specification)?}))
        }).collect::<Result<Vec<_>>>()?;
        Ok(
            json!({"schema":1,"resources":resources,"next_after":next_after,"production_execution":false}),
        )
    }
    fn reserved_status(
        &self,
        journal: &Journal,
        owner: &Owner,
        reserved: Reservation,
    ) -> Result<Value> {
        match reserved {
            Reservation::Completed { resource, .. }
            | Reservation::Pending { resource, .. }
            | Reservation::Reserved { resource, .. } => self.status(journal, owner, &resource),
        }
    }
    fn status(&self, journal: &Journal, owner: &Owner, id: &str) -> Result<Value> {
        let record = journal.resource(owner, id)?;
        if record.phase == Phase::Closed {
            let path = self.result_path(id, "status.json");
            AclPolicy::private_vm().verify(&path, false)?;
            let status: Value = serde_json::from_slice(&deployment::read_bytes(&path, 65536)?)?;
            ensure!(
                status["job_id"] == id
                    && status["native_owner"] == serde_json::to_value(record.owner)?,
                "native terminal status binding differs"
            );
            return Ok(status);
        }
        if let Some(running) = self
            .running
            .as_ref()
            .filter(|running| running.request.resource_id == id)
        {
            return Ok(
                json!({"job_id":id,"kind":running.request.kind(),"state":if running.failed.is_some() {"lost"} else {"running"},"native_host_pid":running.host.pid(),"error":running.failed,"cleanup_pending":running.failed.is_some(),"production_execution":false}),
            );
        }
        Ok(
            json!({"job_id":id,"kind":job_kind(&record)?,"state":"lost","cleanup_pending":true,"error":"native host ownership was interrupted; cleanup reconciliation is required","production_execution":false}),
        )
    }
    pub(crate) fn cancel_active(&mut self) -> Result<()> {
        if let Some(id) = self
            .session
            .as_ref()
            .map(|debugger| debugger.request.target.resource_id.clone())
        {
            self.cancel(&id)?;
            self.session.as_mut().unwrap().request_cancel();
        }
        if let Some(running) = &self.running {
            self.cancel(&running.request.resource_id)?;
        }
        Ok(())
    }
    fn cancel(&self, id: &str) -> Result<()> {
        let path = self.request_path(id, "cancel");
        let private = AclPolicy::private_vm();
        if path.exists() {
            private.verify(&path, false)?;
        } else {
            deployment::publish_json(&path, &json!({"resource_id":id}), &private)?;
        }
        Ok(())
    }
    pub(crate) fn poll(&mut self, journal: &mut Journal) -> Result<()> {
        if self.recovery.is_some() {
            return self.poll_recovery(journal);
        }
        if let Some(debugger) = &self.session {
            return if debugger.needs_recovery() {
                self.recover(journal)
            } else {
                Ok(())
            };
        }
        let Some(running) = &self.running else {
            return self.recover(journal);
        };
        if running.failed.is_some() {
            return self.recover(journal);
        }
        if !running.host.exited() && Instant::now() < running.deadline {
            return Ok(());
        }
        let result = self.settle(journal);
        if let Err(error) = result {
            // Keep both held controls and the journal blocker. A log or exited
            // leader does not establish scratch/profile cleanup.
            self.running.as_mut().unwrap().failed =
                Some(format!("{error:#}").chars().take(2048).collect());
        }
        Ok(())
    }
    fn settle(&mut self, journal: &mut Journal) -> Result<()> {
        let running = self.running.as_ref().context("native host vanished")?;
        ensure!(
            running.host.stop(Duration::from_secs(10))? == 0,
            "native host failed or exceeded preparation deadline; cleanup remains unconfirmed"
        );
        let request = running.request.clone();
        self.finish_result(journal, &request)?;
        self.running = None;
        Ok(())
    }
    fn finish_result(&self, journal: &mut Journal, request: &Request) -> Result<()> {
        if request.replay.is_some() {
            return self.finish_replay_result(journal, request);
        }
        let id = &request.resource_id;
        let private = AclPolicy::private_vm();
        let path = self.result_path(id, "json");
        private.verify(&path, false)?;
        let bytes = deployment::read_bytes(&path, 12 * 1024 * 1024)?;
        let result: Value = serde_json::from_slice(&bytes)?;
        let streams = crate::resource_process::output(request, &result)?;
        for (name, data) in ["stdout", "stderr"].into_iter().zip(&streams) {
            self.retain(&self.result_path(id, name), data)?;
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
            "duration_secs":result["duration_secs"],"output_bytes_retained":{"stdout":streams[0].len(),"stderr":streams[1].len()},"output_truncated":reason == "output_limit" || streams.iter().any(|stream|stream.len()>4096)},"production_execution":false});
        self.retain(
            &self.result_path(id, "status.json"),
            &serde_json::to_vec(&value)?,
        )?;
        journal.start_closing(id)?;
        journal.close(
            id,
            Cleanup {
                jobs_empty: true,
                profiles_removed: true,
                scratch_removed: true,
            },
        )?;
        Ok(())
    }
    fn retain(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        let policy = AclPolicy::private_vm();
        if path.try_exists()? {
            policy.verify(path, false)?;
            ensure!(
                deployment::read_bytes(path, bytes.len() as u64)? == bytes,
                "native published artifact cannot be replaced"
            );
            Ok(())
        } else {
            policy.publish(path, bytes)
        }
    }
    fn request_path(&self, id: &str, suffix: &str) -> PathBuf {
        self.root
            .join("metadata/resource-requests")
            .join(format!("{id}.{suffix}"))
    }
    fn result_path(&self, id: &str, suffix: &str) -> PathBuf {
        self.root
            .join("metadata/resource-results")
            .join(format!("{id}.{suffix}"))
    }
    fn artifact(&self, id: &str, name: &str) -> Result<Value> {
        let suffix = artifact_suffix(name)?;
        let path = self.result_path(id, &suffix);
        AclPolicy::private_vm().verify(&path, false)?;
        let data = deployment::read_bytes(&path, 32 * 1024 * 1024)?;
        Ok(
            json!({"path":format!("native-results/{id}/{name}"),"size":data.len(),"sha256":hex::encode(Sha256::digest(&data))}),
        )
    }
    fn read_artifact(
        &self,
        journal: &Journal,
        owner: &Owner,
        method: &str,
        params: &Map<String, Value>,
    ) -> Result<Value> {
        let path = params["path"]
            .as_str()
            .context("native artifact selector is required")?;
        let parts: Vec<_> = path.split('/').collect();
        ensure!(
            parts.len() == 3 && parts[0] == "native-results",
            "invalid native artifact selector"
        );
        let record = journal.resource(owner, parts[1])?;
        let data = if let Some(sequence) = parts[2].strip_prefix("transcript-") {
            ensure!(
                matches!(
                    record.kind,
                    ResourceKind::Debugger | ResourceKind::Interactive
                ),
                "native transcript requires its session resource"
            );
            let parsed: u64 = sequence.parse()?;
            ensure!(
                parsed.to_string() == sequence,
                "noncanonical native transcript sequence"
            );
            let prefix = crate::session_request::prefix(record.kind)?;
            let request_path = self.request_path(parts[1], &format!("{prefix}.json"));
            AclPolicy::private_vm().verify(&request_path, false)?;
            let request: crate::session_request::Request =
                serde_json::from_slice(&deployment::read_bytes(&request_path, 65536)?)?;
            request.validate()?;
            let spec: Specification =
                serde_json::from_value(record.specification["process"].clone())?;
            ensure!(
                request.target.resource_id == record.id
                    && request.target.owner == record.owner
                    && spec.matches(&request.target)
                    && request.kind() == record.kind
                    && if record.kind == ResourceKind::Interactive {
                        record.specification["native_interactive_schema"] == request.schema
                    } else {
                        record.specification["native_debugger_schema"] == request.schema
                            && record.specification["debugger"]["mode"]
                                == if request.read_only() {
                                    "dump"
                                } else {
                                    "launch"
                                }
                            && record.specification["debugger"]["dump"].as_str()
                                == request.dump().map(|file| file.path.as_str())
                    },
                "native debugger transcript request differs from its journal intent"
            );
            crate::session_artifacts::read(
                &self.root,
                parts[1],
                parsed,
                &crate::inputs::digest(&request)?,
            )?
        } else {
            ensure!(
                record.phase == Phase::Closed,
                "native artifacts require completed cleanup"
            );
            let suffix = if parts[2] == "receipt" && record.kind != ResourceKind::Process {
                let status = self.session_terminal(&record, &AclPolicy::private_vm())?;
                if status["state"] == "lost" {
                    artifact_suffix(
                        status["result"]["recovery_artifact"]
                            .as_str()
                            .context("debugger recovery selector missing")?,
                    )?
                } else {
                    format!(
                        "{}-result.json",
                        crate::session_request::prefix(record.kind)?
                    )
                }
            } else {
                artifact_suffix(parts[2])?
            };
            let source = self.result_path(parts[1], &suffix);
            AclPolicy::private_vm().verify(&source, false)?;
            deployment::read_bytes(&source, 32 * 1024 * 1024)?
        };
        let mut artifact =
            json!({"path":path,"size":data.len(),"sha256":hex::encode(Sha256::digest(&data))});
        if method == "file.stat" {
            return Ok(artifact);
        }
        if method == "file.get" {
            artifact["data_base64"] = json!(STANDARD.encode(data));
            return Ok(artifact);
        }
        let integer = |key: &str, default: u64| -> Result<usize> {
            Ok(usize::try_from(match params.get(key) {
                None => default,
                Some(value) => value
                    .as_u64()
                    .context("native read offset/limit must be an unsigned integer")?,
            })?)
        };
        let offset = integer("offset", 0)?;
        let maximum = integer("max_bytes", 1024 * 1024)?;
        ensure!(
            offset <= data.len() && (1..=1024 * 1024).contains(&maximum),
            "native read range exceeds bounds"
        );
        let chunk = &data[offset..data.len().min(offset.saturating_add(maximum))];
        Ok(
            json!({"path":path,"offset":offset,"size":data.len(),"data_base64":STANDARD.encode(chunk),"chunk_sha256":hex::encode(Sha256::digest(chunk))}),
        )
    }
}

fn artifact_suffix(name: &str) -> Result<String> {
    if let Some(trial) = name.strip_prefix("trial-") {
        let (trial, kind) = trial.split_once('-').unwrap_or((trial, "record"));
        let index: u16 = trial.parse()?;
        ensure!(
            index < 4096 && index.to_string() == trial,
            "invalid replay artifact index"
        );
        let suffix = match kind {
            "record" => "json",
            "input" => "input",
            "mcp" => "mcp",
            "observation" => "observation.json",
            "debug-log" => "debug-log",
            "debug-stdout" => "debug-stdout",
            "debug-stderr" => "debug-stderr",
            _ => anyhow::bail!("invalid minimization artifact kind"),
        };
        return Ok(format!("trial-{index}.{suffix}"));
    }
    match name {
        "stdout" | "stderr" | "minimized" => Ok(name.into()),
        "receipt" => Ok("json".into()),
        _ => {
            let attempt = name
                .strip_prefix("recovery-")
                .context("unknown native artifact kind")?;
            ensure!(
                attempt.len() == 32
                    && attempt
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
                "invalid native recovery artifact identity"
            );
            Ok(format!("{name}.json"))
        }
    }
}

#[cfg(test)]
#[path = "resource_runtime_tests.rs"]
mod tests;
