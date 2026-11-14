//! Authoritative operation contracts. Platform registries bind these operations to handlers.
use crate::{params::*, *};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// Tool-specific observations retain the full third-party evidence payload.
pub type RawEvidence = Map<String, Value>;
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    Read,
    Reconcile,
    Once,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResourceDemand {
    None,
    Execution,
}

macro_rules! methods {
    ($($variant:ident => ($name:literal, $input:ty, $output:ty, $effect:ident, $resource:ident)),* $(,)?) => {
        #[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum Method { $(#[serde(rename = $name)] $variant),* }
        impl Method {
            pub const ALL: &'static [Self] = &[$(Self::$variant),*];
            pub const fn name(self) -> &'static str { match self { $(Self::$variant => $name),* } }
            pub fn parse(name: &str) -> anyhow::Result<Self> {
                match name { $($name => Ok(Self::$variant),)* _ => anyhow::bail!("unsupported worker method: {name}") }
            }
            pub const fn effect(self) -> Effect { match self { $(Self::$variant => Effect::$effect),* } }
            pub const fn resources(self) -> ResourceDemand { match self { $(Self::$variant => ResourceDemand::$resource),* } }
            pub fn input_schema(self) -> schemars::Schema { match self { $(Self::$variant => schemars::schema_for!($input)),* } }
            pub fn output_schema(self) -> schemars::Schema { match self { $(Self::$variant => schemars::schema_for!($output)),* } }
            pub fn decode(self, value: Value) -> anyhow::Result<Request> {
                Ok(match self { $(Self::$variant => Request::$variant(serde_json::from_value(value)?)),* })
            }
        }
        pub enum Request { $($variant($input)),* }
        impl Request {
            pub fn method(&self) -> Method { match self { $(Self::$variant(_) => Method::$variant),* } }
            pub fn parameters(&self) -> anyhow::Result<Value> { Ok(match self { $(Self::$variant(value) => serde_json::to_value(value)?),* }) }
        }
    }
}
methods! {
    Ping => ("ping", Empty, PingResult, Read, None),
    Doctor => ("doctor", Empty, DoctorResult, Read, None),
    LeaseAcquire => ("environment.lease.acquire", LeaseAcquire, EnvironmentLease, Reconcile, None),
    LeaseRelease => ("environment.lease.release", LeaseRelease, LeaseReleased, Reconcile, None),
    Shutdown => ("worker.shutdown", Empty, ShutdownResult, Reconcile, None),
    FilePut => ("file.put", FilePut, FileReceipt, Reconcile, None),
    FileGet => ("file.get", FileGet, FileContents, Read, None),
    FileStat => ("file.stat", FileStat, FileReference, Read, None),
    FileRead => ("file.read", FileRead, FileChunk, Read, None),
    UploadBegin => ("file.upload.begin", UploadBegin, TransferStarted, Reconcile, None),
    UploadWrite => ("file.upload.write", UploadWrite, TransferStatus, Reconcile, None),
    UploadStatus => ("file.upload.status", Transfer, TransferStatus, Read, None),
    UploadCommit => ("file.upload.commit", Transfer, FileReceipt, Reconcile, None),
    JobStart => ("job.start", JobStartArgs, JobStatus, Reconcile, Execution),
    JobStatus => ("job.status", JobStatusArgs, JobStatus, Read, None),
    JobCancel => ("job.cancel", JobCancelArgs, JobStatus, Reconcile, None),
    JobArtifacts => ("job.artifacts", JobArtifactsArgs, Vec<FileReference>, Read, None),
    JobList => ("job.list", Empty, Vec<JobStatus>, Read, None),
    ResourceList => ("resource.list", ResourceList, RawEvidence, Read, None),
    DebugStart => ("debug.start", DebugStartArgs, SessionHandle, Once, Execution),
    DebugInspect => ("debug.inspect", DebugInspectArgs, RawEvidence, Read, None),
    DebugControl => ("debug.control", DebugControlArgs, RawEvidence, Once, None),
    DebugCommand => ("debug.command", DebugCommandArgs, RawEvidence, Once, None),
    DebugClose => ("debug.close", DebugCloseArgs, SessionHandle, Reconcile, None),
    DebugProfile => ("debug.profile", DebugCloseArgs, RawEvidence, Read, None),
    DebugEvents => ("debug.events", DebugCloseArgs, RawEvidence, Read, None),
    DebugSnapshot => ("debug.snapshot", CrashSnapshotArgs, RawEvidence, Read, None),
    InteractStart => ("interact.start", InteractiveStartArgs, SessionHandle, Once, Execution),
    InteractWrite => ("interact.write", InteractiveSendArgs, RawEvidence, Once, None),
    InteractProfile => ("interact.profile", InteractiveCloseArgs, RawEvidence, Read, None),
    InteractClose => ("interact.close", InteractiveCloseArgs, SessionHandle, Once, None),
    ExecRun => ("exec.run", ExecCommandArgs, RawEvidence, Once, Execution),
    PythonRun => ("py.run", PyRunArgs, JobStatus, Once, Execution),
    CrashReplay => ("crash.replay", CrashReplayArgs, JobStatus, Reconcile, Execution),
    InputMinimize => ("input.minimize", InputMinimizeArgs, JobStatus, Reconcile, Execution),
    PocVerify => ("poc.verify", PocVerifyRequest, JobStatus, Reconcile, Execution),
    FuzzStart => ("fuzz.start", FuzzStartArgs, JobStatus, Reconcile, Execution),
    FuzzStatus => ("fuzz.status", FuzzStatusArgs, JobStatus, Read, None),
    FuzzStop => ("fuzz.stop", JobCancelArgs, JobStatus, Reconcile, None),
    AngrSolve => ("angr.solve", AngrSolveArgs, JobStatus, Reconcile, Execution),
    EmulateRun => ("emu.run", EmulateRunArgs, RawEvidence, Once, Execution),
    LibcIdentify => ("libc.identify", LibcIdentifyArgs, RawEvidence, Read, Execution),
    SeccompAnalyze => ("seccomp.analyze", SeccompAnalyzeArgs, RawEvidence, Read, Execution),
    CyclicOffset => ("cyclic.offset", CyclicOffsetArgs, RawEvidence, Read, Execution),
    CoreSnapshot => ("core.snapshot", CrashSnapshotArgs, RawEvidence, Read, None),
    RuntimeProfile => ("runtime.profile", RuntimeProfileArgs, RawEvidence, Read, None),
    RuntimePrepare => ("runtime.prepare", RuntimeProfileArgs, RawEvidence, Reconcile, None),
    NetworkProfile => ("network.profile", NetworkProfile, RawEvidence, Read, None),
    GhidraQuery => ("ghidra.query", GhidraQuery, RawEvidence, Reconcile, Execution),
}

/// Context is decoded separately so operation schemas can reject unknown fields.
pub fn decode(method: Method, mut params: Value) -> anyhow::Result<(RequestContext, Request)> {
    let object = params
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("worker parameters must be an object"))?;
    let mut context = Map::new();
    for key in ["request_id", "call_id", "_binding", "_lease"] {
        if let Some(value) = object.remove(key) {
            context.insert(key.into(), value);
        }
    }
    let context = Value::Object(context);
    validators()
        .context
        .validate(&context)
        .map_err(|e| anyhow::anyhow!("invalid request context: {e}"))?;
    validators().inputs[&method]
        .validate(&params)
        .map_err(|e| anyhow::anyhow!("invalid {} parameters: {e}", method.name()))?;
    let context: RequestContext = serde_json::from_value(context)?;
    if let Some(id) = &context.request_id {
        anyhow::ensure!(!id.is_empty() && id.len() <= 256, "invalid request_id");
    }
    Ok((context, method.decode(params)?))
}

struct Validators {
    context: jsonschema::Validator,
    inputs: BTreeMap<Method, jsonschema::Validator>,
    outputs: BTreeMap<Method, jsonschema::Validator>,
}
fn validators() -> &'static Validators {
    static VALIDATORS: std::sync::LazyLock<Validators> = std::sync::LazyLock::new(|| {
        let compile = |schema: schemars::Schema| {
            jsonschema::validator_for(&schema.to_value())
                .expect("generated contract is a valid schema")
        };
        Validators {
            context: compile(schemars::schema_for!(RequestContext)),
            inputs: Method::ALL
                .iter()
                .map(|m| (*m, compile(m.input_schema())))
                .collect(),
            outputs: Method::ALL
                .iter()
                .map(|m| (*m, compile(m.output_schema())))
                .collect(),
        }
    });
    &VALIDATORS
}

pub fn validate_result(method: Method, value: &Value) -> anyhow::Result<()> {
    validators().outputs[&method]
        .validate(value)
        .map_err(|e| anyhow::anyhow!("invalid {} result: {e}", method.name()))
}

pub fn contract() -> Value {
    let methods: BTreeMap<_, _> = Method::ALL
        .iter()
        .map(|method| {
            (
                method.name(),
                serde_json::json!({
                    "params": method.input_schema(), "result": method.output_schema(),
                    "effect": method.effect(), "resources": method.resources(),
                }),
            )
        })
        .collect();
    serde_json::json!({"protocol": PROTOCOL, "methods": methods,
        "context": schemars::schema_for!(RequestContext),
        "error": schemars::schema_for!(WorkerErrorData),
        "identity": schemars::schema_for!(WorkerIdentity),
        "capacity": schemars::schema_for!(WorkerCapacity),
        "session": schemars::schema_for!(SessionHandle),
        "capabilities": schemars::schema_for!(RegisteredMethods)})
}
