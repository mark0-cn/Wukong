//! Built-in schemas and visibility metadata share one registry.
use super::*;

pub(super) struct Spec {
    pub(super) parse: fn(Value) -> Result<Value>,
    pub(super) definition: ToolDefinition,
    pub(super) side_effecting: bool,
    pub(super) root_only: bool,
    pub(super) child_only: bool,
}
fn spec<A: serde::de::DeserializeOwned + serde::Serialize + schemars::JsonSchema>(
    name: &str,
    description: &str,
    side_effecting: bool,
    target: bool,
) -> Spec {
    Spec {
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
        root_only: false,
        child_only: false,
    }
}

pub(super) fn builtins() -> &'static [Spec] {
    static CATALOG: std::sync::LazyLock<Vec<Spec>> = std::sync::LazyLock::new(build_catalog);
    &CATALOG
}

fn build_catalog() -> Vec<Spec> {
    let mut specs = vec![
        spec::<args::CreateAgentArgs>(
            "create_agent",
            "Create an asynchronous focused child and return immediately; inherited context is optional. wait_for_predecessors gates admission with dependency_policy (default all_success). Failed requirements block execution and notify the parent.",
            true,
            false,
        ),
        spec::<args::ViewAgentGraphArgs>(
            "view_agent_graph",
            "List this assessment's agent tree, scopes, states and results.",
            false,
            false,
        ),
        spec::<args::SendMessageToAgentArgs>(
            "send_message_to_agent",
            "Queue information, a question, reply, result or blocked notice. Information does not interrupt waits. Only follow_up_agent reopens completed work. Queued/included is not proof of processing.",
            true,
            false,
        ),
        spec::<args::FollowUpAgentArgs>(
            "follow_up_agent",
            "Explicitly assign a follow-up; reopens completed work in a new generation while preserving history. Use recipient_generation to guard against stale assignments.",
            true,
            false,
        ),
        spec::<args::MessageStatusArgs>(
            "message_status",
            "Inspect a sent/received message, its delivery state and correlated reply.",
            false,
            false,
        ),
        spec::<args::WaitForAgentsArgs>(
            "wait_for_agents",
            "Park on selected agents with policy all_terminal (default), all_success or any_success; return a satisfied/blocked outcome or timeout. Empty list means children.",
            true,
            false,
        ),
        spec::<args::StopAgentArgs>(
            "stop_agent",
            "Stop yourself or your descendants, with a recorded reason.",
            true,
            false,
        ),
        spec::<args::AgentFinishArgs>(
            "agent_finish",
            "Finish a child task with a structured result accounting for its contract, evidence and gaps. summary is shorthand for tasks without required outputs and is ignored when result is provided (result.answer is the canonical summary). Active descendants must finish first.",
            true,
            false,
        ),
        spec::<args::FinishScanArgs>(
            "finish_scan",
            "Root only: finish the assessment after children and coverage are resolved.",
            true,
            false,
        ),
        spec::<args::ThreatModelGetArgs>(
            "threat_model_get",
            "Read the shared threat model documents.",
            false,
            false,
        ),
        spec::<args::ThreatModelSaveArgs>(
            "threat_model_save",
            "Create a shared threat model with a stable document ID.",
            true,
            false,
        ),
        spec::<args::ThreatModelAmendArgs>(
            "threat_model_amend",
            "Append a durable amendment to a shared threat model.",
            true,
            false,
        ),
        spec::<args::NoteSaveArgs>(
            "note_save",
            "Save a shared note linked to this task.",
            true,
            false,
        ),
        spec::<args::NotesListArgs>(
            "notes_list",
            "List shared notes for this assessment.",
            false,
            false,
        ),
        spec::<args::ContextSearchArgs>(
            "context_search",
            "Search same-run shared notes, todos and optional threat models by text, kind, author scope or agent_id. Returns bounded previews and next_offset; use context_read for exact content including amendments.",
            false,
            false,
        ),
        spec::<args::ContextReadArgs>(
            "context_read",
            "Read a bounded character range of an exact same-run shared document as JSON, including amendments. Use kind and id from context_search; follow content.next_offset until the required content is complete.",
            false,
            false,
        ),
        spec::<args::TodoSaveArgs>(
            "todo_save",
            "Create a shared to-do item; status defaults to pending.",
            true,
            false,
        ),
        spec::<args::TodoUpdateArgs>(
            "todo_update",
            "Update the body or status of an existing shared to-do.",
            true,
            false,
        ),
        spec::<args::TodosListArgs>("todos_list", "List shared to-do items.", false, false),
        spec::<args::CoverageRecordArgs>(
            "coverage_record",
            "Record inspected scope, outcome and supporting evidence. needs_follow_up blocks root completion.",
            true,
            false,
        ),
        spec::<args::CoverageUpdateArgs>(
            "coverage_update",
            "Update a coverage record's outcome, notes and evidence.",
            true,
            false,
        ),
        spec::<args::CoverageListArgs>(
            "coverage_list",
            "List all recorded assessment coverage.",
            false,
            false,
        ),
        spec::<args::FindingFileArgs>(
            "finding_file",
            "File a suspected or reproduced finding with evidence. Verified impact requires a separate immutable verifier record. severity critical/high requires a reproduced-or-better stage.",
            true,
            false,
        ),
        spec::<args::FindingUpdateArgs>(
            "finding_update",
            "Update an existing finding. Impact promotion is enforced using independent execution evidence.",
            true,
            false,
        ),
        spec::<args::FindingsListArgs>(
            "findings_list",
            "List this assessment's findings and recorded stages.",
            false,
            false,
        ),
        spec::<args::FindingGetArgs>(
            "finding_get",
            "Read a finding and its independent verification record.",
            false,
            false,
        ),
        spec::<args::VerificationRecordArgs>(
            "verification_record",
            "Verifier only: ingest a completed poc_verify job, its raw evidence and real trial counts. A crash verifies only crash behavior.",
            true,
            true,
        ),
        spec::<args::EvidenceReadArgs>(
            "evidence_read",
            "Read a bounded byte range from an integrity-checked evidence artifact. Non-UTF8 is base64.",
            false,
            false,
        ),
        spec::<args::LoadSkillArgs>(
            "load_skill",
            "Activate a named capability or method (built-in or configured custom skill). Role instructions root/analyst/verifier are automatic and cannot be loaded here. Initial and active capabilities share a limit of 5 distinct names. Exact content/hash is persisted for the next fresh model boundary; replaces an initial copy of the same skill and survives compaction/resume. Loading never changes role, permissions or task completion criteria.",
            true,
            false,
        ),
        spec::<args::ThinkArgs>(
            "think",
            "Record a concise audited scratch note for this agent. The original content remains in tool-call history and may be inherited by children; this is neither a private channel nor a context-saving mechanism. Use note_save for shared conclusions.",
            true,
            false,
        ),
        spec::<args::BinaryInspectArgs>(
            "binary_inspect",
            "Inspect the exact run target, architecture, sections, imports, symbols and mitigations.",
            false,
            true,
        ),
        spec::<args::BinaryStringsArgs>(
            "binary_strings",
            "Search printable strings with file-offset address references and pagination.",
            false,
            true,
        ),
        spec::<args::FunctionsSearchArgs>(
            "functions_search",
            "Ghidra: search analyzed functions, paginated.",
            false,
            true,
        ),
        spec::<args::FunctionDecompileArgs>(
            "function_decompile",
            "Ghidra: decompile a function by MODULE-RELATIVE offset.",
            false,
            true,
        ),
        spec::<args::XrefsQueryArgs>(
            "xrefs_query",
            "Ghidra: references to/from a MODULE-RELATIVE offset.",
            false,
            true,
        ),
        spec::<args::RuntimeDoctorArgs>(
            "runtime_doctor",
            "Inspect the configured worker's real platform, versions and available capabilities.",
            false,
            true,
        ),
        spec::<args::StageTargetArgs>(
            "stage_target",
            "Copy the run's SHA256-verified original target into the worker's immutable files area.",
            true,
            true,
        ),
        spec::<args::FilePutArgs>(
            "file_put",
            "Create an immutable worker file from base64 bytes or an existing evidence artifact.",
            true,
            true,
        ),
        spec::<args::FileGetArgs>(
            "file_get",
            "Retrieve a declared in-run worker file and persist original bytes as evidence.",
            false,
            true,
        ),
        spec::<args::ExecCommandArgs>(
            "exec_command",
            "Run ONE bounded shell command synchronously in the worker and return its stdout/stderr/exit code in the result. Prefer this for quick inspection and for stdin-driven PoCs: pipe the payload directly, e.g. \"printf '<payload>' | {target-path-or-command}\" or \"python3 -c '...'\". Long-running work must go to job_start with output redirected to a log file instead of blocking here. Reads RE_AGENT_STATE_DIR/RE_AGENT_JOB_DIR from the environment.",
            true,
            true,
        ),
        spec::<args::JobStartArgs>(
            "job_start",
            "Start a persistent process job in the configured worker; returns a job_id immediately. argv entries may use {target} as a placeholder for the staged target path (resolved by the harness and staged in isolated jobs); worker paths like files/... resolve only as direct argv entries, never inside shell strings. Jobs may read RE_AGENT_STATE_DIR and RE_AGENT_JOB_DIR from the environment.",
            true,
            true,
        ),
        spec::<args::JobStatusArgs>(
            "job_status",
            "Observe an in-run job, refresh persisted status and import raw observations.",
            false,
            true,
        ),
        spec::<args::JobCancelArgs>(
            "job_cancel",
            "Request cancellation of an owned in-run job.",
            true,
            true,
        ),
        spec::<args::JobArtifactsArgs>(
            "job_artifacts",
            "List and import original artifacts produced by an in-run job.",
            false,
            true,
        ),
        spec::<args::FuzzStartArgs>(
            "fuzz_start",
            "Start AFL++ QEMU fuzzing after staging target and nonempty seed files.",
            true,
            true,
        ),
        spec::<args::FuzzStatusArgs>(
            "fuzz_status",
            "Observe a bound AFL++ job and its real statistics/crash files.",
            false,
            true,
        ),
        spec::<args::DebugStartArgs>(
            "debug_start",
            "Start an owned GDB/MI session on the staged target.",
            true,
            true,
        ),
        spec::<args::DebugCommandArgs>(
            "debug_command",
            "Execute exactly one GDB/MI command in your owned session; stable request IDs prevent blind replay.",
            true,
            true,
        ),
        spec::<args::DebugCloseArgs>(
            "debug_close",
            "Close your debugger session, or clean up an in-run session whose owner is terminal.",
            true,
            true,
        ),
        spec::<args::InteractiveStartArgs>(
            "interactive_start",
            "Start a persistent interactive process (REPL-style program) owned by this agent; returns a session_id. Worker paths resolve as direct argv entries.",
            true,
            true,
        ),
        spec::<args::InteractiveSendArgs>(
            "interactive_send",
            "Write bytes to an owned interactive session's stdin and return its newly produced output. The session stays alive between calls.",
            true,
            true,
        ),
        spec::<args::InteractiveCloseArgs>(
            "interactive_close",
            "Close your interactive session and terminate its process tree; also permits cleanup when its in-run owner is terminal.",
            true,
            true,
        ),
        spec::<args::AngrSolveArgs>(
            "angr_solve",
            "Symbolic execution: solve for a concrete stdin input that reaches a module-relative find address while avoiding others. Runs as a background job with a hard timeout; results (input_base64 + exploration stats) arrive via job_status.",
            true,
            true,
        ),
        spec::<args::CrashReplayArgs>(
            "crash_replay",
            "Start a persistent clean-directory crash reproduction job.",
            true,
            true,
        ),
        spec::<args::InputMinimizeArgs>(
            "input_minimize",
            "Start crash-preserving delta reduction using observed crash signatures.",
            true,
            true,
        ),
        spec::<args::PocVerifyArgs>(
            "poc_verify",
            "Independently replay a finding's input in fresh directories; worker observations determine trial success. input_mode selects delivery: \"argv\" (default, AFL-style @@ file path) or \"stdin\" (payload bytes piped to the target's stdin — use this for programs that read(0, ...)).",
            true,
            true,
        ),
    ];
    for item in &mut specs {
        match item.definition.name.as_str() {
            "finish_scan" => item.root_only = true,
            "agent_finish" => item.child_only = true,
            _ => {}
        }
    }
    specs
}
pub(crate) fn child_schema() -> Value {
    rig_core::tool::args_schema::<args::CreateAgentArgs>()
}
