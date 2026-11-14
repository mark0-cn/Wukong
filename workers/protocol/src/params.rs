//! Typed worker operation inputs, shared by model schemas where their inputs coincide.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
macro_rules! choices {
    ($name:ident { $($variant:ident),* $(,)? }) => {
        #[derive(Serialize, Deserialize, JsonSchema)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($variant),* }
    };
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Expected {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout_contains: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr_contains: Option<String>,
}

choices!(ByteOrder { Little, Big });

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InputMode {
    Argv,
    Stdin,
}

choices!(CaptureEncoding { Hex, Little, Big });

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JobStartArgs {
    #[serde(default)]
    pub argv: Vec<String>,
    /// Existing prepared LaunchSpec; omit argv/env/cwd overrides when supplied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_spec: Option<String>,
    /// Admitted input file; LaunchSpec selects stdin_pipe or argv_file delivery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<std::collections::BTreeMap<String, String>>,
    #[schemars(range(min = 1, max = 86400))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    #[schemars(range(min = 1024, max = 8388608))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_limit: Option<u64>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JobStatusArgs {
    #[schemars(length(min = 1))]
    pub job_id: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JobCancelArgs {
    #[schemars(length(min = 1))]
    pub job_id: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JobArtifactsArgs {
    #[schemars(length(min = 1))]
    pub job_id: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FuzzStartArgs {
    #[schemars(length(min = 1))]
    pub target: String,
    #[schemars(length(min = 1))]
    pub seeds: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<std::collections::BTreeMap<String, String>>,
    #[schemars(range(min = 1, max = 86400))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    #[schemars(range(min = 10, max = 60000))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec_timeout_ms: Option<u64>,
    #[schemars(range(min = 16, max = 65536))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_mb: Option<u64>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FuzzStatusArgs {
    #[schemars(length(min = 1))]
    pub job_id: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DebugStartArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<DebugMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<DebugEngine>,
    /// Staged dump artifact; required in dump mode, separate from the target image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dump: Option<String>,
    /// Input artifact for argv_file LaunchSpecs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<String>,
    /// Prepared worker-relative LaunchSpec; shares loader, libraries, cwd and input mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_spec: Option<String>,
    #[schemars(length(min = 1))]
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DebugCommandArgs {
    #[schemars(length(min = 1))]
    pub session_id: String,
    #[schemars(length(min = 1))]
    pub command: String,
    #[schemars(range(min = 1, max = 60))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

choices!(DebugMode { Launch, Dump });
choices!(DebugEngine {
    Auto,
    GdbMi,
    Dbgeng
});
choices!(DebugInspectKind {
    Registers,
    Stack,
    Memory,
    Modules,
    Threads,
    Disassembly,
    Exception,
    State
});
choices!(DebugAction {
    Run,
    Continue,
    Interrupt,
    Step,
    Next,
    Finish,
    BreakpointAdd,
    BreakpointRemove,
    BreakpointEnable,
    BreakpointDisable
});

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DebugInspectArgs {
    pub session_id: String,
    pub kind: DebugInspectKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<u64>,
    #[schemars(range(min = 1, max = 65536))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<u64>,
    #[schemars(range(min = 1, max = 256))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    #[schemars(range(min = 1, max = 60))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DebugControlArgs {
    pub session_id: String,
    pub action: DebugAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<u64>,
    #[schemars(length(min = 1, max = 512))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[schemars(length(min = 1, max = 32))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breakpoint_id: Option<String>,
    #[schemars(range(min = 1, max = 60))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DebugCloseArgs {
    #[schemars(length(min = 1))]
    pub session_id: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InteractiveStartArgs {
    /// Input artifact for argv_file LaunchSpecs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<String>,
    /// Prepared worker-relative LaunchSpec; shares loader, libraries, cwd and input mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_spec: Option<String>,
    pub argv: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<std::collections::BTreeMap<String, String>>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InteractiveSendArgs {
    #[schemars(length(min = 1))]
    pub session_id: String,
    pub data_base64: String,
    #[schemars(range(min = 1, max = 300))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_timeout_secs: Option<u64>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InteractiveCloseArgs {
    #[schemars(length(min = 1))]
    pub session_id: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AngrSolveArgs {
    #[schemars(length(min = 1))]
    pub target: String,
    #[schemars(length(min = 1))]
    pub find: Vec<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avoid: Option<Vec<u64>>,
    #[schemars(range(min = 1, max = 4096))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdin_length: Option<u64>,
    #[schemars(range(min = 1, max = 10000000))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_steps: Option<u64>,
    #[schemars(range(min = 10, max = 3600))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub printable_only: Option<bool>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PyRunArgs {
    #[schemars(length(min = 1, max = 262144))]
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argv: Option<Vec<String>>,
    #[schemars(range(min = 10, max = 3600))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    #[schemars(range(min = 1024, max = 8388608))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_limit: Option<u64>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LibcIdentifyArgs {
    /// Legacy single leak, optionally combined with leaks from the same process.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leaked: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[schemars(length(min = 1, max = 16))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leaks: Option<Vec<LibcLeak>>,
    /// In-run ELF candidate paths. Omission checks only the worker's own libc.
    #[schemars(length(min = 1, max = 16))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidates: Option<Vec<String>>,
    /// Optional staged target for architecture/word-width/byte-order comparison.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LibcLeak {
    pub address: u64,
    #[schemars(length(min = 1, max = 256))]
    pub symbol: String,
    /// Bytes after the named symbol, e.g. an identified return site.
    #[serde(default)]
    pub addend: i64,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SeccompProbe {
    /// AUDIT_ARCH value, not the ELF machine number.
    pub arch: u32,
    pub nr: u32,
    #[schemars(length(max = 6))]
    #[serde(default)]
    pub args: Vec<u64>,
    #[serde(default)]
    pub instruction_pointer: u64,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SeccompAnalyzeArgs {
    /// In-run raw sock_filter array, without a sock_fprog header.
    pub filter: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endianness: Option<ByteOrder>,
    #[schemars(length(max = 64))]
    #[serde(default)]
    pub probes: Vec<SeccompProbe>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EmulateRunArgs {
    #[schemars(length(min = 1))]
    pub target: String,
    pub start: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<u64>,
    #[schemars(range(min = 1, max = 10000000))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub init_regs: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<Vec<Map<String, Value>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dump: Option<Vec<Map<String, Value>>>,
    #[schemars(range(min = 1, max = 600))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CrashReplayArgs {
    /// Prepared worker-relative LaunchSpec; shares loader, libraries, cwd and input mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_spec: Option<String>,
    #[schemars(length(min = 1))]
    pub target: String,
    #[schemars(length(min = 1))]
    pub input: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_mode: Option<InputMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<Expected>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<std::collections::BTreeMap<String, String>>,
    #[schemars(range(min = 1, max = 60))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    #[schemars(range(min = 1, max = 16))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trials: Option<u64>,
    #[schemars(range(min = 1, max = 4096))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_attempts: Option<u64>,
    #[schemars(range(min = 1, max = 3600))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimize_timeout_secs: Option<u64>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InputMinimizeArgs {
    /// Prepared worker-relative LaunchSpec; shares loader, libraries, cwd and input mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_spec: Option<String>,
    #[schemars(length(min = 1))]
    pub target: String,
    /// Explicit weak fallback; default requires module-hash/location crash equivalence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_signal_only: Option<bool>,
    #[schemars(length(min = 1))]
    pub input: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_mode: Option<InputMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<std::collections::BTreeMap<String, String>>,
    #[schemars(range(min = 1, max = 60))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    #[schemars(range(min = 1, max = 4096))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_attempts: Option<u64>,
    #[schemars(range(min = 1, max = 3600))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimize_timeout_secs: Option<u64>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecCommandArgs {
    #[schemars(length(min = 1, max = 65536))]
    pub command: String,
    pub cwd: Option<String>,
    /// Optional staged worker path (from stage_target); the worker places the
    /// resolved file at $TARGET in the command environment.
    pub target: Option<String>,
    pub env: Option<std::collections::BTreeMap<String, String>>,
    #[schemars(range(min = 1, max = 120))]
    pub timeout_secs: Option<u64>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PocVerifyArgs {
    #[schemars(length(min = 1))]
    pub finding_id: String,
    #[serde(flatten)]
    pub input: VerificationInput,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct VerificationInput {
    /// Prepared worker-relative LaunchSpec; shares loader, libraries, cwd and input mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_spec: Option<String>,
    #[schemars(length(min = 1))]
    pub target: String,
    #[schemars(length(min = 1))]
    pub input: String,
    /// Bounded TCP replay through the prepared challenge service. Requires launch_spec;
    /// excludes input_mode/args/env/expected. Input supplies the bytes for send slices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<NetworkReplay>,
    /// How the input reaches the target: "argv" (AFL-style @@ file path,
    /// default) or "stdin" (bytes piped to stdin).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_mode: Option<InputMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<Expected>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<std::collections::BTreeMap<String, String>>,
    #[schemars(range(min = 1, max = 60))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    #[schemars(range(min = 1, max = 16))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trials: Option<u64>,
    #[schemars(range(min = 1, max = 4096))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_attempts: Option<u64>,
    #[schemars(range(min = 1, max = 3600))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimize_timeout_secs: Option<u64>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NetworkReplay {
    #[schemars(length(min = 2, max = 64))]
    pub steps: Vec<NetworkStep>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum NetworkStep {
    Send {
        offset: u64,
        length: u64,
        #[serde(default)]
        patches: Vec<NetworkPatch>,
    },
    Receive {
        until_base64: String,
        max_bytes: u64,
    },
    Capture {
        name: String,
        offset: u64,
        length: u64,
        encoding: CaptureEncoding,
    },
    /// Assert bytes in the latest response after a send. Required as the last step.
    Expect { contains_base64: String },
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NetworkPatch {
    pub offset: u64,
    pub capture: String,
    #[serde(default)]
    pub addend: i64,
    /// 1, 2, 4 or 8 bytes; values that do not fit are rejected.
    pub width: u8,
    pub endianness: ByteOrder,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PocVerifyRequest {
    #[schemars(length(min = 1))]
    pub producer_agent_id: String,
    #[schemars(length(min = 1))]
    pub verifier_agent_id: String,
    pub network_binding: Option<Value>,
    #[serde(flatten)]
    pub input: VerificationInput,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuntimeProfileArgs {
    pub target: String,
    pub libc: Option<String>,
    pub loader: Option<String>,
    /// Owned live debugger session; never an arbitrary PID.
    pub session_id: Option<String>,
    /// Alternatively observe an owned interactive process (e.g. a prepared runtime bundle).
    pub interactive_session_id: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CrashSnapshotArgs {
    /// Choose exactly one: owned stopped debugger session or in-run core file.
    pub session_id: Option<String>,
    pub core: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CyclicOffsetArgs {
    pub pattern: String,
    pub observed_base64: String,
    pub alphabet: String,
    pub n: u64,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Empty {}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FilePut {
    pub path: String,
    pub data_base64: String,
    #[serde(default)]
    pub executable: bool,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FileGet {
    pub path: String,
    pub max_bytes: Option<u64>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FileStat {
    pub path: String,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FileRead {
    pub path: String,
    pub offset: u64,
    pub max_bytes: Option<u64>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UploadBegin {
    pub path: String,
    #[schemars(range(max = 17179869184_u64))]
    pub size: u64,
    #[schemars(regex(pattern = "^[0-9a-f]{64}$"))]
    pub sha256: String,
    #[serde(default)]
    pub executable: bool,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Transfer {
    pub transfer_id: String,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UploadWrite {
    pub transfer_id: String,
    pub offset: u64,
    pub data_base64: String,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LeaseAcquire {
    pub run_id: String,
    pub owner_id: String,
    pub ttl_secs: Option<u64>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NetworkProfile {
    pub target: String,
    pub launch_spec: String,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GhidraQuery {
    pub target: String,
    pub target_sha256: String,
    pub query: Map<String, Value>,
    pub timeout_secs: u64,
    pub analysis_timeout_secs: u64,
    pub max_cpu: u64,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LeaseRelease {
    pub lease: crate::EnvironmentLease,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResourceList {
    pub after: Option<String>,
    #[schemars(range(min = 1, max = 256))]
    pub limit: Option<u64>,
}
