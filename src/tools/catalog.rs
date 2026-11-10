//! Built-in schemas and visibility metadata share one registry.
use super::*;

pub(super) struct Spec {
    pub(super) handler: Handler,
    pub(super) policy: ReplayPolicy,
    pub(super) parse: fn(Value) -> Result<Value>,
    pub(super) definition: ToolDefinition,
    pub(super) side_effecting: bool,
    pub(super) root_only: bool,
    pub(super) child_only: bool,
    pub(super) model_visible: bool,
}
impl Spec {
    pub(super) fn allows(&self, role: Role) -> bool {
        (!self.root_only || role == Role::Root) && (!self.child_only || role != Role::Root)
    }
}

type Handler = for<'a> fn(
    &'a ToolService,
    &'a AgentMetadata,
    &'a str,
    &'a str,
    Value,
) -> futures::future::BoxFuture<'a, Result<Value>>;

#[derive(Clone, Copy)]
enum Visibility {
    All,
    Root,
    Child,
    Internal,
}

fn spec<A: serde::de::DeserializeOwned + serde::Serialize + schemars::JsonSchema>(
    name: &str,
    description: &str,
    policy: ReplayPolicy,
    target: bool,
    handler: Handler,
    visibility: Visibility,
) -> Spec {
    let side_effecting = policy.side_effecting();
    Spec {
        handler,
        policy,
        definition: ToolDefinition::new(
            ToolName::new(name).expect("built-in tool name"),
            description,
            rig_core::tool::args_schema::<A>(),
        )
        .with_annotations(
            ToolAnnotations::default()
                .with_read_only(!side_effecting)
                .with_open_world(target),
        ),
        parse: |value| Ok(serde_json::to_value(serde_json::from_value::<A>(value)?)?),
        side_effecting,
        model_visible: !matches!(visibility, Visibility::Internal),
        root_only: matches!(visibility, Visibility::Root),
        child_only: matches!(visibility, Visibility::Child) || (target && name != "runtime_doctor"),
    }
}

pub(super) fn builtins() -> &'static [Spec] {
    static CATALOG: std::sync::LazyLock<Vec<Spec>> = std::sync::LazyLock::new(build_catalog);
    &CATALOG
}

fn build_catalog() -> Vec<Spec> {
    vec![
        spec::<args::SeccompAnalyzeArgs>(
            "seccomp_analyze",
            "Decode an in-run raw classic-BPF seccomp filter into instructions, branch targets and actions; optionally evaluate concrete architecture/syscall/argument/IP probes. An artifact is not a confirmed installed policy, and probes are not an exhaustive allowlist.",
            ReplayPolicy::ReadOnly,
            true,
            handlers::handle_seccomp_analyze,
            Visibility::Child,
        ),
        spec::<args::MitigationProfileArgs>(
            "mitigation_profile",
            "Separate binary declarations, worker environment support and owned process/thread protection observations; optional address checks the actual containing mapping.",
            ReplayPolicy::ReadOnly,
            true,
            handlers::handle_mitigation_profile,
            Visibility::Child,
        ),
        spec::<args::CrashSnapshotArgs>(
            "crash_snapshot",
            "Capture registers, PC/SP, fault address, instructions, stack and module identities from an owned stopped debugger; alternatively inspect an in-run core file. Strong signatures require observed module hashes. Unknown identity remains unresolved.",
            ReplayPolicy::ReadOnly,
            true,
            handlers::handle_crash_snapshot,
            Visibility::Child,
        ),
        spec::<args::CyclicOffsetArgs>(
            "cyclic_offset",
            "Find a unique offset in an in-run cyclic pattern. Requires the original alphabet/n and exactly n observed bytes in memory order. Rejects mismatched generation parameters; never guesses endianness or truncates a register implicitly.",
            ReplayPolicy::ReadOnly,
            true,
            handlers::handle_crash_snapshot,
            Visibility::Child,
        ),
        spec::<args::RuntimeProfileArgs>(
            "runtime_profile",
            "Inspect SHA256, ELF Build ID and architecture of in-run target/libc/loader files. Optionally observe actual mappings of an owned live debugger session. File candidates never imply a confirmed mapped runtime.",
            ReplayPolicy::ReadOnly,
            true,
            handlers::handle_runtime_profile,
            Visibility::Child,
        ),
        spec::<args::RuntimeProfileArgs>(
            "runtime_prepare",
            "Create an immutable execution copy of supplied target, libc and loader and return direct launch argv. Requires all three ELF files with matching architecture. Does not execute or modify originals; verify actual mappings separately.",
            ReplayPolicy::WorkerIdempotent,
            true,
            handlers::handle_runtime_profile,
            Visibility::Child,
        ),
        spec::<args::FunctionInstructionsArgs>(
            "function_instructions",
            "Ghidra: paginated function instructions and control-flow edges, including calls. Addresses are module-relative and bound to target SHA256. Optional target_evidence_id selects an in-run baseline binary.",
            ReplayPolicy::ReadOnly,
            true,
            handlers::handle_functions_search,
            Visibility::Child,
        ),
        spec::<args::DifferentialTestArgs>(
            "differential_test",
            "Compare the staged target and reference on bounded stdin vectors in an isolated background job. Preserve raw outputs, hashes and mismatches as artifacts. Timeout or truncated output is inconclusive, never a match.",
            ReplayPolicy::WorkerIdempotent,
            true,
            handlers::handle_differential_test,
            Visibility::Child,
        ),
        spec::<args::AnalysisVerifyArgs>(
            "analysis_verify",
            "Verifier only: record an independent general-analysis verdict with expected and observed results, scope, counterexamples and verifier-owned evidence. Does not promote security impact. Records are immutable and appear in shared notes.",
            ReplayPolicy::Transactional,
            false,
            handlers::handle_analysis_verify,
            Visibility::Child,
        ),
        spec::<args::CreateAgentArgs>(
            "create_agent",
            "Create an asynchronous focused child and return immediately; inherited context is optional. wait_for_predecessors gates admission with dependency_policy (default all_success). Failed requirements block execution and notify the parent.",
            ReplayPolicy::Transactional,
            false,
            handlers::handle_create_agent,
            Visibility::Internal,
        ),
        spec::<args::ViewAgentGraphArgs>(
            "view_agent_graph",
            "List this assessment's agent tree, scopes, states and results.",
            ReplayPolicy::ReadOnly,
            false,
            handlers::handle_view_agent_graph,
            Visibility::All,
        ),
        spec::<args::SendMessageToAgentArgs>(
            "send_message_to_agent",
            "Queue information, a question, reply, result or blocked notice. Information does not interrupt waits. Only follow_up_agent reopens completed work. Queued/included is not proof of processing.",
            ReplayPolicy::Transactional,
            false,
            handlers::handle_send_message_to_agent,
            Visibility::All,
        ),
        spec::<args::FollowUpAgentArgs>(
            "follow_up_agent",
            "Explicitly assign a follow-up; reopens completed work in a new generation while preserving history. Use recipient_generation to guard against stale assignments.",
            ReplayPolicy::Transactional,
            false,
            handlers::handle_send_message_to_agent,
            Visibility::All,
        ),
        spec::<args::MessageStatusArgs>(
            "message_status",
            "Inspect a sent/received message, its delivery state and correlated reply.",
            ReplayPolicy::ReadOnly,
            false,
            handlers::handle_message_status,
            Visibility::All,
        ),
        spec::<args::WaitForAgentsArgs>(
            "wait_for_agents",
            "Park on selected agents with policy all_terminal (default), all_success or any_success; return a satisfied/blocked outcome or timeout. Empty list means children.",
            ReplayPolicy::Transactional,
            false,
            handlers::handle_wait_for_agents,
            Visibility::All,
        ),
        spec::<args::StopAgentArgs>(
            "stop_agent",
            "Stop yourself or your descendants, with a recorded reason.",
            ReplayPolicy::Transactional,
            false,
            handlers::handle_stop_agent,
            Visibility::All,
        ),
        spec::<args::AgentFinishArgs>(
            "agent_finish",
            "Finish a child task with a structured result accounting for its contract, evidence and gaps. summary is shorthand for tasks without required outputs and is ignored when result is provided (result.answer is the canonical summary). Active descendants must finish first.",
            ReplayPolicy::Transactional,
            false,
            handlers::handle_agent_finish,
            Visibility::Child,
        ),
        spec::<args::FinishScanArgs>(
            "finish_scan",
            "Root only: finish the assessment after children and coverage are resolved.",
            ReplayPolicy::Transactional,
            false,
            handlers::handle_agent_finish,
            Visibility::Root,
        ),
        spec::<args::ThreatModelGetArgs>(
            "threat_model_get",
            "Read the shared threat model documents.",
            ReplayPolicy::ReadOnly,
            false,
            handlers::handle_threat_model_get,
            Visibility::All,
        ),
        spec::<args::ThreatModelSaveArgs>(
            "threat_model_save",
            "Create a shared threat model with a stable document ID.",
            ReplayPolicy::Transactional,
            false,
            handlers::handle_threat_model_save,
            Visibility::All,
        ),
        spec::<args::ThreatModelAmendArgs>(
            "threat_model_amend",
            "Append a durable amendment to a shared threat model.",
            ReplayPolicy::Transactional,
            false,
            handlers::handle_threat_model_amend,
            Visibility::All,
        ),
        spec::<args::NoteSaveArgs>(
            "note_save",
            "Save a shared note linked to this task.",
            ReplayPolicy::Transactional,
            false,
            handlers::handle_threat_model_save,
            Visibility::All,
        ),
        spec::<args::NotesListArgs>(
            "notes_list",
            "List shared notes for this assessment.",
            ReplayPolicy::ReadOnly,
            false,
            handlers::handle_notes_list,
            Visibility::All,
        ),
        spec::<args::ContextSearchArgs>(
            "context_search",
            "Search same-run shared notes, todos and optional threat models by text, kind, author scope or agent_id. Returns bounded previews and next_offset; use context_read for exact content including amendments.",
            ReplayPolicy::ReadOnly,
            false,
            handlers::handle_context_search,
            Visibility::All,
        ),
        spec::<args::ContextReadArgs>(
            "context_read",
            "Read a bounded character range of an exact same-run shared document as JSON, including amendments. Use kind and id from context_search; follow content.next_offset until the required content is complete.",
            ReplayPolicy::ReadOnly,
            false,
            handlers::handle_context_read,
            Visibility::All,
        ),
        spec::<args::TodoSaveArgs>(
            "todo_save",
            "Create a shared to-do item; status defaults to pending.",
            ReplayPolicy::Transactional,
            false,
            handlers::handle_threat_model_save,
            Visibility::All,
        ),
        spec::<args::TodoUpdateArgs>(
            "todo_update",
            "Update the body or status of an existing shared to-do.",
            ReplayPolicy::Transactional,
            false,
            handlers::handle_threat_model_amend,
            Visibility::All,
        ),
        spec::<args::TodosListArgs>(
            "todos_list",
            "List shared to-do items.",
            ReplayPolicy::ReadOnly,
            false,
            handlers::handle_todos_list,
            Visibility::All,
        ),
        spec::<args::CoverageRecordArgs>(
            "coverage_record",
            "Record inspected scope, outcome and supporting evidence. needs_follow_up blocks root completion.",
            ReplayPolicy::Transactional,
            false,
            handlers::handle_coverage_record,
            Visibility::All,
        ),
        spec::<args::CoverageUpdateArgs>(
            "coverage_update",
            "Update a coverage record's outcome, notes and evidence.",
            ReplayPolicy::Transactional,
            false,
            handlers::handle_coverage_record,
            Visibility::All,
        ),
        spec::<args::CoverageListArgs>(
            "coverage_list",
            "List all recorded assessment coverage.",
            ReplayPolicy::ReadOnly,
            false,
            handlers::handle_coverage_list,
            Visibility::All,
        ),
        spec::<args::FindingFileArgs>(
            "finding_file",
            "File a suspected or reproduced finding with evidence. Verified impact requires a separate immutable verifier record. severity critical/high requires a reproduced-or-better stage.",
            ReplayPolicy::Transactional,
            false,
            handlers::handle_finding_file,
            Visibility::All,
        ),
        spec::<args::FindingUpdateArgs>(
            "finding_update",
            "Update an existing finding. Impact promotion is enforced using independent execution evidence.",
            ReplayPolicy::Transactional,
            false,
            handlers::handle_finding_file,
            Visibility::All,
        ),
        spec::<args::FindingsListArgs>(
            "findings_list",
            "List this assessment's findings and recorded stages.",
            ReplayPolicy::ReadOnly,
            false,
            handlers::handle_findings_list,
            Visibility::All,
        ),
        spec::<args::FindingGetArgs>(
            "finding_get",
            "Read a finding and its independent verification record.",
            ReplayPolicy::ReadOnly,
            false,
            handlers::handle_finding_get,
            Visibility::All,
        ),
        spec::<args::VerificationRecordArgs>(
            "verification_record",
            "Verifier only: ingest a completed poc_verify job, its raw evidence and real trial counts. A crash verifies only crash behavior.",
            ReplayPolicy::Transactional,
            true,
            handlers::handle_verification_record,
            Visibility::All,
        ),
        spec::<args::EvidenceReadArgs>(
            "evidence_read",
            "Read a bounded byte range from an integrity-checked evidence artifact. Non-UTF8 is base64.",
            ReplayPolicy::ReadOnly,
            false,
            handlers::handle_evidence_read,
            Visibility::All,
        ),
        spec::<args::LoadSkillArgs>(
            "load_skill",
            "Activate a named capability or method (built-in or configured custom skill). Role instructions root/analyst/verifier are automatic and cannot be loaded here. Initial and active capabilities share a limit of 5 distinct names. Exact content/hash is persisted for the next fresh model boundary; replaces an initial copy of the same skill and survives compaction/resume. Loading never changes role, permissions or task completion criteria.",
            ReplayPolicy::Transactional,
            false,
            handlers::handle_load_skill,
            Visibility::All,
        ),
        spec::<args::ThinkArgs>(
            "think",
            "Record a concise audited scratch note for this agent. The original content remains in tool-call history and may be inherited by children; this is neither a private channel nor a context-saving mechanism. Use note_save for shared conclusions.",
            ReplayPolicy::Transactional,
            false,
            handlers::handle_think,
            Visibility::All,
        ),
        spec::<args::BinaryInspectArgs>(
            "binary_inspect",
            "Inspect the exact run target, architecture, sections, imports, symbols and mitigations.",
            ReplayPolicy::ReadOnly,
            true,
            handlers::handle_binary_inspect,
            Visibility::All,
        ),
        spec::<args::BinaryStringsArgs>(
            "binary_strings",
            "Search printable strings with file-offset address references and pagination.",
            ReplayPolicy::ReadOnly,
            true,
            handlers::handle_binary_strings,
            Visibility::All,
        ),
        spec::<args::FunctionsSearchArgs>(
            "functions_search",
            "Ghidra: search analyzed functions, paginated.",
            ReplayPolicy::ReadOnly,
            true,
            handlers::handle_functions_search,
            Visibility::All,
        ),
        spec::<args::FunctionDecompileArgs>(
            "function_decompile",
            "Ghidra: decompile a function by MODULE-RELATIVE offset.",
            ReplayPolicy::ReadOnly,
            true,
            handlers::handle_functions_search,
            Visibility::All,
        ),
        spec::<args::XrefsQueryArgs>(
            "xrefs_query",
            "Ghidra: references to/from a MODULE-RELATIVE offset.",
            ReplayPolicy::ReadOnly,
            true,
            handlers::handle_functions_search,
            Visibility::All,
        ),
        spec::<args::RuntimeDoctorArgs>(
            "runtime_doctor",
            "Inspect the configured worker's real platform, versions and available capabilities.",
            ReplayPolicy::ReadOnly,
            true,
            handlers::handle_runtime_doctor,
            Visibility::All,
        ),
        spec::<args::StageTargetArgs>(
            "stage_target",
            "Copy the run's SHA256-verified original target into the worker's immutable files area.",
            ReplayPolicy::WorkerIdempotent,
            true,
            handlers::handle_stage_target,
            Visibility::All,
        ),
        spec::<args::FilePutArgs>(
            "file_put",
            "Create an immutable worker file from base64 bytes (data_base64), UTF-8 text (text — preferred for scripts: no base64 encoding needed) or an existing evidence artifact. evidence_id must be the CONTENT evidence id — the record whose bytes you actually want (a tool result's content_evidence_id), NOT the tool-result envelope id: passing the envelope id writes the envelope JSON, not the bytes. Use executable=true when the file will be executed as a program.",
            ReplayPolicy::WorkerIdempotent,
            true,
            handlers::handle_file_put,
            Visibility::All,
        ),
        spec::<args::FileGetArgs>(
            "file_get",
            "Retrieve a declared in-run worker file and persist original bytes as evidence.",
            ReplayPolicy::ReadOnly,
            true,
            handlers::handle_file_get,
            Visibility::All,
        ),
        spec::<args::ExecCommandArgs>(
            "exec_command",
            "Run ONE bounded shell command synchronously in the worker and return its stdout/stderr/exit code in the result. The command runs in a fresh temporary directory: pass target=<staged path from stage_target> and the resolved file is available at $TARGET in the environment (\"printf '<payload>' | $TARGET\"). Prefer this for quick inspection and stdin-driven PoCs; long-running work must go to job_start with output redirected to a log file instead of blocking here. Worker paths (files/...) resolve only as direct argv entries, never inside shell strings; use $WUKONG_STATE_DIR for the state root.",
            ReplayPolicy::WorkerIdempotent,
            true,
            handlers::handle_job_start,
            Visibility::All,
        ),
        spec::<args::JobStartArgs>(
            "job_start",
            "Start a persistent process job in the configured worker; returns a job_id immediately. argv entries may use {target} as a placeholder for the staged target path (resolved by the harness and staged in isolated jobs); worker paths like files/... resolve only as direct argv entries, never inside shell strings. Jobs may read WUKONG_STATE_DIR and WUKONG_JOB_DIR from the environment.",
            ReplayPolicy::WorkerIdempotent,
            true,
            handlers::handle_job_start,
            Visibility::All,
        ),
        spec::<args::JobStatusArgs>(
            "job_status",
            "Observe an in-run job, refresh persisted status and import raw observations.",
            ReplayPolicy::ReadOnly,
            true,
            handlers::handle_job_start,
            Visibility::All,
        ),
        spec::<args::JobCancelArgs>(
            "job_cancel",
            "Request cancellation of an owned in-run job.",
            ReplayPolicy::WorkerIdempotent,
            true,
            handlers::handle_job_start,
            Visibility::All,
        ),
        spec::<args::JobArtifactsArgs>(
            "job_artifacts",
            "List and import original artifacts produced by an in-run job.",
            ReplayPolicy::ReadOnly,
            true,
            handlers::handle_job_start,
            Visibility::All,
        ),
        spec::<args::FuzzStartArgs>(
            "fuzz_start",
            "Start AFL++ QEMU fuzzing after staging target and nonempty seed files.",
            ReplayPolicy::WorkerIdempotent,
            true,
            handlers::handle_job_start,
            Visibility::All,
        ),
        spec::<args::FuzzStatusArgs>(
            "fuzz_status",
            "Observe a bound AFL++ job and its real statistics/crash files.",
            ReplayPolicy::ReadOnly,
            true,
            handlers::handle_job_start,
            Visibility::All,
        ),
        spec::<args::DebugStartArgs>(
            "debug_start",
            "Start an owned GDB/MI session on the staged target.",
            ReplayPolicy::WorkerIdempotent,
            true,
            handlers::handle_job_start,
            Visibility::All,
        ),
        spec::<args::DebugCommandArgs>(
            "debug_command",
            "Execute exactly one GDB/MI command in your owned session; stable request IDs prevent blind replay.",
            ReplayPolicy::WorkerIdempotent,
            true,
            handlers::handle_job_start,
            Visibility::All,
        ),
        spec::<args::DebugCloseArgs>(
            "debug_close",
            "Close your debugger session, or clean up an in-run session whose owner is terminal.",
            ReplayPolicy::WorkerIdempotent,
            true,
            handlers::handle_job_start,
            Visibility::All,
        ),
        spec::<args::InteractiveStartArgs>(
            "interactive_start",
            "Start a persistent interactive process (REPL-style program) owned by this agent; returns a session_id. Worker paths resolve as direct argv entries.",
            ReplayPolicy::WorkerIdempotent,
            true,
            handlers::handle_job_start,
            Visibility::All,
        ),
        spec::<args::InteractiveSendArgs>(
            "interactive_send",
            "Write bytes to an owned interactive session's stdin and return its newly produced output. The session stays alive between calls.",
            ReplayPolicy::WorkerIdempotent,
            true,
            handlers::handle_job_start,
            Visibility::All,
        ),
        spec::<args::InteractiveCloseArgs>(
            "interactive_close",
            "Close your interactive session and terminate its process tree; also permits cleanup when its in-run owner is terminal.",
            ReplayPolicy::WorkerIdempotent,
            true,
            handlers::handle_job_start,
            Visibility::All,
        ),
        spec::<args::AngrSolveArgs>(
            "angr_solve",
            "Bounded symbolic input reconstruction for stdin validation paths. Supply module-relative success/failure addresses and expected input length. Returns a background job with candidate input and exploration statistics. Prefer direct inversion for simple reversible transforms; slice or use explicit constraints for large state spaces. Always replay candidate bytes on the real target.",
            ReplayPolicy::WorkerIdempotent,
            true,
            handlers::handle_job_start,
            Visibility::All,
        ),
        spec::<args::PyRunArgs>(
            "py_run",
            "Run a Python script in the worker in ONE step: pass the source text directly (no base64, no file_put). The worker syntax-checks it and returns the compiler error inline before spending a job; then runs it as a background job (job_status for stdout/stderr). Optional argv entries arrive as sys.argv[1..]; an optional target=<staged path> is appended LAST to argv and staged automatically in isolated jobs — read it from sys.argv[-1]. Prefer this over file_put+job_start for any script longer than one line.",
            ReplayPolicy::WorkerIdempotent,
            true,
            handlers::handle_job_start,
            Visibility::All,
        ),
        spec::<args::LibcIdentifyArgs>(
            "libc_identify",
            "Compare same-process symbol leaks against up to 16 supplied in-run ELF libc candidates, or only the worker's libc when omitted. Preserve candidate hashes, Build IDs, architecture, inconsistent leaks and ambiguous matches. Candidates never confirm the mapped runtime; no match describes only the searched corpus.",
            ReplayPolicy::ReadOnly,
            true,
            handlers::handle_libc_identify,
            Visibility::All,
        ),
        spec::<args::EmulateRunArgs>(
            "emulate_run",
            "Bounded Unicorn emulation of the staged target from a start address: maps the ELF segments plus a stack, executes up to count instructions or until the PC reaches the until address (whichever first), and returns registers plus requested memory dumps (data_base64). The target never runs natively — this is a pure simulation for VM-style or hand-crafted code (interpreter loops, x87/FPU blocks) without hand-building Unicorn harnesses. Use init_regs for register preconditions and memory to inject bytes. The watchdog timeout bounds runaway loops; unsupported instructions report the failing address in error.",
            ReplayPolicy::ReadOnly,
            true,
            handlers::handle_emulate_run,
            Visibility::All,
        ),
        spec::<args::CrashReplayArgs>(
            "crash_replay",
            "Start a persistent clean-directory crash reproduction job.",
            ReplayPolicy::WorkerIdempotent,
            true,
            handlers::handle_job_start,
            Visibility::All,
        ),
        spec::<args::InputMinimizeArgs>(
            "input_minimize",
            "Minimize a crashing input with fresh module-hash, PC-offset and frame signatures; requires GDB. allow_signal_only explicitly permits weaker signal-only equivalence. Poll job_status and collect job_artifacts.",
            ReplayPolicy::WorkerIdempotent,
            true,
            handlers::handle_job_start,
            Visibility::All,
        ),
        spec::<args::PocVerifyArgs>(
            "poc_verify",
            "Independently replay a finding in fresh trial directories. Use argv/stdin delivery, or a bounded declarative network plan with the host-prepared launch_spec. Network trials use fresh TCP connections, capture/patch ASLR-dependent values and hash raw transcripts; server state is not reset and deployed runtime identity remains unconfirmed. Worker observations determine success.",
            ReplayPolicy::WorkerIdempotent,
            true,
            handlers::handle_job_start,
            Visibility::All,
        ),
    ]
}
pub(crate) fn child_schema() -> Value {
    rig_core::tool::args_schema::<args::CreateAgentArgs>()
}

#[cfg(test)]
mod role_tests {
    use super::*;
    #[test]
    fn target_operations_are_never_root_tools() {
        for name in [
            "binary_inspect",
            "function_decompile",
            "exec_command",
            "py_run",
            "debug_start",
            "poc_verify",
        ] {
            let spec = builtins()
                .iter()
                .find(|s| s.definition.name.as_str() == name)
                .unwrap();
            assert!(spec.child_only, "{name} must be delegated");
        }
        for name in ["view_agent_graph", "note_save", "evidence_read"] {
            assert!(
                !builtins()
                    .iter()
                    .find(|s| s.definition.name.as_str() == name)
                    .unwrap()
                    .child_only
            );
        }
    }
}
