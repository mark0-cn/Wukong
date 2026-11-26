//! Built-in schemas and visibility metadata share one registry.
use super::requirements::Requirement;
use super::*;
use wukong_worker_protocol::Method;

pub(super) struct Spec {
    pub(super) handler: Handler,
    pub(super) display_title: &'static str,
    pub(super) policy: ReplayPolicy,
    pub(super) parse: fn(Value) -> Result<Value>,
    pub(super) definition: ToolDefinition,
    pub(super) side_effecting: bool,
    pub(super) root_only: bool,
    pub(super) child_only: bool,
    pub(super) verifier_only: bool,
    pub(super) model_visible: bool,
    pub(super) requirement: super::requirements::Requirement,
}
impl Spec {
    pub(super) fn allows(&self, role: Role) -> bool {
        (!self.root_only || role == Role::Root)
            && (!self.child_only || role != Role::Root)
            && (!self.verifier_only || role == Role::Verifier)
    }
}

type Handler = for<'a> fn(
    &'a ToolContext,
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
    Verifier,
    Internal,
}

#[expect(
    clippy::too_many_arguments,
    reason = "each declarative registry row owns its schema, presentation, handler and permission policy"
)]
fn spec<A: serde::de::DeserializeOwned + serde::Serialize + schemars::JsonSchema>(
    name: &str,
    display_title: &'static str,
    description: &str,
    policy: ReplayPolicy,
    target: bool,
    handler: Handler,
    visibility: Visibility,
    requirement: Requirement,
) -> Spec {
    let side_effecting = policy.side_effecting();
    Spec {
        handler,
        display_title,
        policy,
        definition: ToolDefinition::new(
            ToolName::new(name).expect("built-in tool name"),
            description,
            input_contracts::schema(name, rig_core::tool::args_schema::<A>()),
        )
        .with_annotations(
            ToolAnnotations::default()
                .with_read_only(!side_effecting)
                .with_open_world(target),
        ),
        parse: |value| Ok(serde_json::to_value(serde_json::from_value::<A>(value)?)?),
        side_effecting,
        model_visible: !matches!(visibility, Visibility::Internal),
        requirement,
        root_only: matches!(visibility, Visibility::Root),
        verifier_only: matches!(visibility, Visibility::Verifier),
        child_only: matches!(visibility, Visibility::Child) || target,
    }
}

pub(super) fn builtins() -> &'static [Spec] {
    static CATALOG: std::sync::LazyLock<Vec<Spec>> = std::sync::LazyLock::new(build_catalog);
    &CATALOG
}

fn build_catalog() -> Vec<Spec> {
    vec![
        spec::<args::ResourceReadArgs>(
            "resource_read",
            "tool-resource-read",
            "Read an owned job stdout/stderr or session transcript in bounded byte pages. Reuse next_offset; at_snapshot_end may become readable later, while eof requires a terminal resource. Each chunk is preserved as hashed evidence. A worker may publish outputs only after completion; missing streams are explicit errors.",
            ReplayPolicy::ReadOnly,
            true,
            |services, agent, call, name, args| {
                handlers::handle_resource_read(&services.execution, agent, call, name, args)
            },
            Visibility::Child,
            Requirement::Worker(&[Method::FileRead]),
        ),
        spec::<args::LaunchPrepareArgs>(
            "launch_prepare",
            "tool-launch-prepare",
            "Prepare an immutable hash-bound LaunchSpec from the primary target, selected directory bundle, staged target/dependencies or Linux target/libc/loader. For a bundle, source.entry and source.members choose the executable and member subset. Own args, cwd, environment, input mode and timeout here; reuse launch_spec in jobs and sessions without overrides. Preparation does not execute the target.",
            ReplayPolicy::WorkerIdempotent,
            true,
            |services, agent, call, name, args| {
                handlers::handle_launch_prepare(&services.execution, agent, call, name, args)
            },
            Visibility::Child,
            Requirement::Worker(&[
                Method::FilePut,
                Method::FileStat,
                Method::FileRead,
                Method::UploadBegin,
                Method::UploadWrite,
                Method::UploadCommit,
            ]),
        ),
        spec::<args::ToolCatalogArgs>(
            "tool_catalog",
            "tool-tool-catalog",
            "Query bounded tool groups, current selection and last-observed backend availability. Follow next_cursor with after. Selection never changes roles.",
            ReplayPolicy::ReadOnly,
            false,
            |services, agent, call, name, args| {
                handlers::handle_tool_selection(&services.coordination, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Controller,
        ),
        spec::<args::ToolSelectArgs>(
            "tool_select",
            "tool-tool-select",
            "Replace optional tool groups for subsequent model requests. Core and resource lifecycle tools stay available; external selects configured MCP tools.",
            ReplayPolicy::Transactional,
            false,
            |services, agent, call, name, args| {
                handlers::handle_tool_selection(&services.coordination, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Controller,
        ),
        spec::<args::EvidenceSearchArgs>(
            "evidence_search",
            "tool-evidence-search",
            "Find same-run evidence metadata by producer, target hash, content hash, job, media type or source. Bounded stable ID pages; follow next_cursor using after. Read content with evidence_read.",
            ReplayPolicy::ReadOnly,
            false,
            |services, agent, call, name, args| {
                handlers::handle_inventory(&services.evidence, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Controller,
        ),
        spec::<args::ResourceListArgs>(
            "resource_list",
            "tool-resource-list",
            "List your owned jobs, debugger and interactive sessions with last-observed state. Bounded ID pages; follow next_cursor using after. This query does not grant cleanup authority or discover foreign worker resources.",
            ReplayPolicy::ReadOnly,
            false,
            |services, agent, call, name, args| {
                handlers::handle_inventory(&services.evidence, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Controller,
        ),
        spec::<args::ArtifactValidateArgs>(
            "artifact_validate",
            "tool-artifact-validate",
            "Verifier only: independently test self_check_v1 Python or Shell script predicates with normal, abnormal-exit, signal, timeout, missing, truncated and contradictory observations. Bind the producer content evidence, verifier's successful same-target --verify baseline job, and its imported baseline.json. Starts a trusted bounded driver job; observe it before claiming acceptance. Tests simulated predicates, not target behavior or collector wiring.",
            ReplayPolicy::WorkerIdempotent,
            true,
            |services, agent, call, name, args| {
                handlers::handle_artifact_validate(&services.execution, agent, call, name, args)
            },
            Visibility::Verifier,
            Requirement::ScriptValidation,
        ),
        spec::<args::ArtifactStageArgs>(
            "artifact_stage",
            "tool-artifact-stage",
            "Stage a hash-verified same-run evidence artifact into your assigned environment using bounded resumable transfer; returns the worker file and original evidence ID.",
            ReplayPolicy::WorkerIdempotent,
            true,
            |services, agent, call, name, args| {
                handlers::handle_artifact_stage(&services.execution, agent, call, name, args)
            },
            Visibility::Child,
            Requirement::Worker(&[
                Method::UploadBegin,
                Method::UploadWrite,
                Method::UploadCommit,
            ]),
        ),
        spec::<args::EnvironmentListArgs>(
            "environment_list",
            "tool-environment-list",
            "List registered analysis environments without connecting; configuration is not proof of readiness.",
            ReplayPolicy::ReadOnly,
            false,
            |services, agent, call, name, args| {
                handlers::handle_environment_list(&services.environments, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Controller,
        ),
        spec::<args::EnvironmentDoctorArgs>(
            "environment_doctor",
            "tool-environment-doctor",
            "Probe a registered environment's real platform and tools; children may probe only their assigned environment.",
            ReplayPolicy::ReadOnly,
            false,
            |services, agent, call, name, args| {
                handlers::handle_environment_doctor(&services.environments, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Probe,
        ),
        spec::<args::SeccompAnalyzeArgs>(
            "seccomp_analyze",
            "tool-seccomp-analyze",
            "Decode an in-run raw classic-BPF seccomp filter into instructions, branch targets and actions; optionally evaluate concrete architecture/syscall/argument/IP probes. An artifact is not a confirmed installed policy, and probes are not an exhaustive allowlist.",
            ReplayPolicy::ReadOnly,
            true,
            |services, agent, call, name, args| {
                handlers::handle_seccomp_analyze(&services.execution, agent, call, name, args)
            },
            Visibility::Child,
            Requirement::Worker(&[Method::SeccompAnalyze]),
        ),
        spec::<args::MitigationProfileArgs>(
            "mitigation_profile",
            "tool-mitigation-profile",
            "Separate binary declarations, worker environment support and owned process/thread protection observations; optional address checks the actual containing mapping.",
            ReplayPolicy::ReadOnly,
            true,
            |services, agent, call, name, args| {
                handlers::handle_mitigation_profile(&services.execution, agent, call, name, args)
            },
            Visibility::Child,
            Requirement::Worker(&[Method::RuntimeProfile]),
        ),
        spec::<args::CrashSnapshotArgs>(
            "crash_snapshot",
            "tool-crash-snapshot",
            "Capture registers, PC/SP, fault address, instructions, stack and module identities from an owned stopped debugger; alternatively inspect an in-run core file. Strong signatures require observed module hashes. Unknown identity remains unresolved.",
            ReplayPolicy::ReadOnly,
            true,
            |services, agent, call, name, args| {
                handlers::handle_crash_snapshot(&services.execution, agent, call, name, args)
            },
            Visibility::Child,
            Requirement::Worker(&[Method::CoreSnapshot]),
        ),
        spec::<args::CyclicOffsetArgs>(
            "cyclic_offset",
            "tool-cyclic-offset",
            "Find a unique offset in an in-run cyclic pattern. Requires the original alphabet/n and exactly n observed bytes in memory order. Rejects mismatched generation parameters; never guesses endianness or truncates a register implicitly.",
            ReplayPolicy::ReadOnly,
            true,
            |services, agent, call, name, args| {
                handlers::handle_crash_snapshot(&services.execution, agent, call, name, args)
            },
            Visibility::Child,
            Requirement::Worker(&[Method::CyclicOffset]),
        ),
        spec::<args::RuntimeProfileArgs>(
            "runtime_profile",
            "tool-runtime-profile",
            "Inspect SHA256, ELF Build ID and architecture of in-run target/libc/loader files. Optionally observe actual mappings of an owned live debugger session. File candidates never imply a confirmed mapped runtime.",
            ReplayPolicy::ReadOnly,
            true,
            |services, agent, call, name, args| {
                handlers::handle_runtime_profile(&services.execution, agent, call, name, args)
            },
            Visibility::Child,
            Requirement::Worker(&[Method::RuntimeProfile]),
        ),
        spec::<args::FunctionInstructionsArgs>(
            "function_instructions",
            "tool-function-instructions",
            "Ghidra: paginated function instructions and control-flow edges, including calls. Addresses are module-relative and bound to target SHA256. Optional target_ref={kind:evidence,evidence_id:...} selects an in-run baseline binary.",
            ReplayPolicy::ReadOnly,
            true,
            |services, agent, call, name, args| {
                handlers::handle_functions_search(&services.analysis, agent, call, name, args)
            },
            Visibility::Child,
            Requirement::Ghidra,
        ),
        spec::<args::TraceCompareArgs>(
            "trace_compare",
            "tool-trace-compare",
            "Compare two caller-owned trace_run evidence pairs entirely offline. Both run.json and events.jsonl must be imported from distinct jobs. Compatible, complete nonzero traces return match/mismatch with typed integer, buffer, event-order and semantic process differences; incompatible or incomplete inputs never return match. Full bounded comparison details are retained as trace.comparison evidence.",
            ReplayPolicy::Transactional,
            false,
            |services, agent, call, name, args| {
                handlers::handle_trace_compare(&services.evidence, agent, call, name, args)
            },
            Visibility::Child,
            Requirement::Controller,
        ),
        spec::<args::DifferentialTestArgs>(
            "differential_test",
            "tool-differential-test",
            "Compare two prepared LaunchSpecs on 1..8 same-run input evidence files. Each launch must record a 1..10 second timeout. Runs ordinary owned jobs and preserves original status/output evidence. Choose exact_output, exit_status or stdout_json and expected exit codes explicitly. Missing, timed-out, truncated or invalid JSON observations are inconclusive. May take up to 300 seconds; resources remain discoverable with resource_list.",
            ReplayPolicy::WorkerIdempotent,
            true,
            |services, agent, call, name, args| {
                handlers::handle_differential_test(&services.execution, agent, call, name, args)
            },
            Visibility::Child,
            Requirement::Worker(&[
                Method::JobStart,
                Method::FilePut,
                Method::FileStat,
                Method::FileRead,
                Method::JobStatus,
                Method::JobArtifacts,
            ]),
        ),
        spec::<args::AnalysisVerifyArgs>(
            "analysis_verify",
            "tool-analysis-verify",
            "Verifier only: record claim-level checks, limitations and verifier-owned evidence. Supported covers only listed checks; reproduction checks require the exact artifact and verifier job with status evidence. Does not certify the whole report or promote security impact. Records are immutable shared notes. Validator integration checks bind the real --verify job and an independent review of shared collector/evaluator wiring. Original owned job-status evidence is resolved automatically; errors identify invalid check indexes.",
            ReplayPolicy::Transactional,
            false,
            |services, agent, call, name, args| {
                handlers::handle_analysis_verify(&services.evidence, agent, call, name, args)
            },
            Visibility::Verifier,
            Requirement::Controller,
        ),
        spec::<args::CreateAgentArgs>(
            "create_agent",
            "tool-create-agent",
            "Create an asynchronous focused child and return immediately. New tasks default to isolated context (inherit_context=false); include a bounded brief and evidence references. wait_for_predecessors gates admission with dependency_policy (default all_success). Failed requirements block execution and notify the parent.",
            ReplayPolicy::Transactional,
            false,
            |services, agent, call, name, args| {
                handlers::handle_create_agent(&services.coordination, agent, call, name, args)
            },
            Visibility::Internal,
            Requirement::Controller,
        ),
        spec::<args::ViewAgentGraphArgs>(
            "view_agent_graph",
            "tool-view-agent-graph",
            "Query compact paginated agent states with canonical IDs (default 8 per page; follow next_offset). Pass agent_id for one agent's full task, contract and result. Unique prefixes of at least 8 characters are accepted within this run; use returned full IDs for coordination.",
            ReplayPolicy::ReadOnly,
            false,
            |services, agent, call, name, args| {
                handlers::handle_view_agent_graph(&services.coordination, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Controller,
        ),
        spec::<args::SendMessageToAgentArgs>(
            "send_message_to_agent",
            "tool-send-message-to-agent",
            "Queue information, a question, reply, result or blocked notice. Information does not interrupt waits. Only follow_up_agent reopens completed work. Queued/included is not proof of processing.",
            ReplayPolicy::Transactional,
            false,
            |services, agent, call, name, args| {
                handlers::handle_send_message_to_agent(
                    &services.coordination,
                    agent,
                    call,
                    name,
                    args,
                )
            },
            Visibility::All,
            Requirement::Controller,
        ),
        spec::<args::FollowUpAgentArgs>(
            "follow_up_agent",
            "tool-follow-up-agent",
            "Explicitly assign a follow-up; reopens completed work in a new generation while preserving history. Use recipient_generation to guard against stale assignments.",
            ReplayPolicy::Transactional,
            false,
            |services, agent, call, name, args| {
                handlers::handle_send_message_to_agent(
                    &services.coordination,
                    agent,
                    call,
                    name,
                    args,
                )
            },
            Visibility::All,
            Requirement::Controller,
        ),
        spec::<args::MessageStatusArgs>(
            "message_status",
            "tool-message-status",
            "Inspect a sent/received message, its delivery state and correlated reply.",
            ReplayPolicy::ReadOnly,
            false,
            |services, agent, call, name, args| {
                handlers::handle_message_status(&services.coordination, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Controller,
        ),
        spec::<args::WaitForAgentsArgs>(
            "wait_for_agents",
            "tool-wait-for-agents",
            "Park on selected agents with policy all_terminal (default), all_success or any_success; return a satisfied/blocked outcome or timeout. Empty list means children.",
            ReplayPolicy::Transactional,
            false,
            |services, agent, call, name, args| {
                handlers::handle_wait_for_agents(&services.coordination, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Controller,
        ),
        spec::<args::StopAgentArgs>(
            "stop_agent",
            "tool-stop-agent",
            "Stop yourself or your descendants, with a recorded reason.",
            ReplayPolicy::Transactional,
            false,
            |services, agent, call, name, args| {
                handlers::handle_stop_agent(&services.coordination, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Controller,
        ),
        spec::<args::AgentFinishArgs>(
            "agent_finish",
            "tool-agent-finish",
            "Finish a child task with a structured result accounting for its contract, evidence and gaps. result.answer is the canonical summary. Active descendants must finish first.",
            ReplayPolicy::Transactional,
            false,
            |services, agent, call, name, args| {
                handlers::handle_agent_finish(&services.coordination, agent, call, name, args)
            },
            Visibility::Child,
            Requirement::Controller,
        ),
        spec::<args::FinishScanArgs>(
            "finish_scan",
            "tool-finish-scan",
            "Root only: finish the assessment after children and coverage are resolved.",
            ReplayPolicy::Transactional,
            false,
            |services, agent, call, name, args| {
                handlers::handle_agent_finish(&services.coordination, agent, call, name, args)
            },
            Visibility::Root,
            Requirement::Controller,
        ),
        spec::<args::ThreatModelSaveArgs>(
            "threat_model_save",
            "tool-threat-model-save",
            "Create a shared threat model with a stable document ID.",
            ReplayPolicy::Transactional,
            false,
            |services, agent, call, name, args| {
                handlers::handle_threat_model_save(&services.documents, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Controller,
        ),
        spec::<args::ThreatModelAmendArgs>(
            "threat_model_amend",
            "tool-threat-model-amend",
            "Append a durable amendment to a shared threat model.",
            ReplayPolicy::Transactional,
            false,
            |services, agent, call, name, args| {
                handlers::handle_threat_model_amend(&services.documents, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Controller,
        ),
        spec::<args::NoteSaveArgs>(
            "note_save",
            "tool-note-save",
            "Save a shared note linked to this task.",
            ReplayPolicy::Transactional,
            false,
            |services, agent, call, name, args| {
                handlers::handle_threat_model_save(&services.documents, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Controller,
        ),
        spec::<args::ContextSearchArgs>(
            "context_search",
            "tool-context-search",
            "Search same-run shared notes, todos and optional threat models by text, kind, author scope or agent_id. Returns bounded previews and next_offset; use context_read for exact content including amendments.",
            ReplayPolicy::ReadOnly,
            false,
            |services, agent, call, name, args| {
                handlers::handle_context_search(&services.documents, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Controller,
        ),
        spec::<args::ContextReadArgs>(
            "context_read",
            "tool-context-read",
            "Read a bounded character range of an exact same-run shared document as JSON, including amendments. Use kind and id from context_search; follow content.next_offset until the required content is complete.",
            ReplayPolicy::ReadOnly,
            false,
            |services, agent, call, name, args| {
                handlers::handle_context_read(&services.documents, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Controller,
        ),
        spec::<args::TodoSaveArgs>(
            "todo_save",
            "tool-todo-save",
            "Create a shared to-do item; status defaults to pending.",
            ReplayPolicy::Transactional,
            false,
            |services, agent, call, name, args| {
                handlers::handle_threat_model_save(&services.documents, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Controller,
        ),
        spec::<args::TodoUpdateArgs>(
            "todo_update",
            "tool-todo-update",
            "Update the body or status of an existing shared to-do.",
            ReplayPolicy::Transactional,
            false,
            |services, agent, call, name, args| {
                handlers::handle_threat_model_amend(&services.documents, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Controller,
        ),
        spec::<args::CoverageRecordArgs>(
            "coverage_record",
            "tool-coverage-record",
            "Record or update inspected scope, outcome and supporting evidence. Repeating the same normalized scope/kind for an agent updates one row; needs_follow_up blocks root completion. Root may pass supersedes for same-run rows with matching normalized scope/kind and terminal owners, retaining them as nonblocking superseded history.",
            ReplayPolicy::Transactional,
            false,
            |services, agent, call, name, args| {
                handlers::handle_coverage_record(&services.evidence, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Controller,
        ),
        spec::<args::CoverageListArgs>(
            "coverage_list",
            "tool-coverage-list",
            "List bounded coverage summaries filtered by agent, scope or outcome. Follow next_cursor with after; notes are previews and full_evidence_id retains the full page.",
            ReplayPolicy::ReadOnly,
            false,
            |services, agent, call, name, args| {
                handlers::handle_coverage_list(&services.evidence, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Controller,
        ),
        spec::<args::FindingFileArgs>(
            "finding_file",
            "tool-finding-file",
            "File a suspected or reproduced finding with evidence. Verified impact requires a separate immutable verifier record. severity critical/high requires a reproduced-or-better stage.",
            ReplayPolicy::Transactional,
            false,
            |services, agent, call, name, args| {
                handlers::handle_finding_file(&services.evidence, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Controller,
        ),
        spec::<args::FindingUpdateArgs>(
            "finding_update",
            "tool-finding-update",
            "Update an existing finding. Impact promotion is enforced using independent execution evidence.",
            ReplayPolicy::Transactional,
            false,
            |services, agent, call, name, args| {
                handlers::handle_finding_file(&services.evidence, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Controller,
        ),
        spec::<args::FindingsListArgs>(
            "findings_list",
            "tool-findings-list",
            "List bounded finding summaries filtered by agent, stage or severity. Follow next_cursor with after; use finding_get for full details.",
            ReplayPolicy::ReadOnly,
            false,
            |services, agent, call, name, args| {
                handlers::handle_findings_list(&services.evidence, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Controller,
        ),
        spec::<args::FindingGetArgs>(
            "finding_get",
            "tool-finding-get",
            "Read a finding and its independent verification record.",
            ReplayPolicy::ReadOnly,
            false,
            |services, agent, call, name, args| {
                handlers::handle_finding_get(&services.evidence, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Controller,
        ),
        spec::<args::VerificationRecordArgs>(
            "verification_record",
            "tool-verification-record",
            "Verifier only: ingest a completed poc_verify job, its raw evidence and real trial counts. A crash verifies only crash behavior. Passing records atomically link the finding's first valid verification_id without promoting its stage; failed verdicts do not set that pointer, and a different valid link is never overwritten.",
            ReplayPolicy::Transactional,
            true,
            |services, agent, call, name, args| {
                handlers::handle_verification_record(&services.execution, agent, call, name, args)
            },
            Visibility::Verifier,
            Requirement::Worker(&[Method::JobStatus, Method::JobArtifacts]),
        ),
        spec::<args::EvidenceReadArgs>(
            "evidence_read",
            "tool-evidence-read",
            "Read a bounded byte range from an integrity-checked evidence artifact. UTF-8 pages align to character boundaries and report requested/actual offsets; invalid UTF-8 is base64.",
            ReplayPolicy::ReadOnly,
            false,
            |services, agent, call, name, args| {
                handlers::handle_evidence_read(&services.evidence, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Controller,
        ),
        spec::<args::LoadSkillArgs>(
            "load_skill",
            "tool-load-skill",
            "Activate a named capability or method (built-in or configured custom skill). Role instructions root/analyst/verifier are automatic and cannot be loaded here. Initial and active capabilities share a limit of 5 distinct names. Exact content/hash is persisted for the next fresh model boundary; replaces an initial copy of the same skill and survives compaction/resume. Loading never changes role, permissions or task completion criteria.",
            ReplayPolicy::Transactional,
            false,
            |services, agent, call, name, args| {
                handlers::handle_load_skill(&services.coordination, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Controller,
        ),
        spec::<args::BinaryInspectArgs>(
            "binary_inspect",
            "tool-binary-inspect",
            "Inspect binary architecture, sections, imports, symbols and mitigations. Optional target_ref selects primary or same-run evidence; omitted selects the primary target.",
            ReplayPolicy::ReadOnly,
            true,
            |services, agent, call, name, args| {
                handlers::handle_binary_inspect(&services.analysis, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Controller,
        ),
        spec::<args::BinaryStringsArgs>(
            "binary_strings",
            "tool-binary-strings",
            "Search printable strings with file-offset address references and pagination. Optional target_ref selects primary or same-run binary evidence.",
            ReplayPolicy::ReadOnly,
            true,
            |services, agent, call, name, args| {
                handlers::handle_binary_strings(&services.analysis, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Controller,
        ),
        spec::<args::FunctionsSearchArgs>(
            "functions_search",
            "tool-functions-search",
            "Ghidra: search analyzed functions, paginated.",
            ReplayPolicy::ReadOnly,
            true,
            |services, agent, call, name, args| {
                handlers::handle_functions_search(&services.analysis, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Ghidra,
        ),
        spec::<args::FunctionDecompileArgs>(
            "function_decompile",
            "tool-function-decompile",
            "Ghidra: decompile using a target-bound module_relative AddressRef. Copy binary_inspect addresses; a link-time VA must subtract image_base. File offsets, runtime addresses and page numbers are not module offsets.",
            ReplayPolicy::ReadOnly,
            true,
            |services, agent, call, name, args| {
                handlers::handle_functions_search(&services.analysis, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Ghidra,
        ),
        spec::<args::XrefsQueryArgs>(
            "xrefs_query",
            "tool-xrefs-query",
            "Ghidra: references to/from a target-bound module_relative AddressRef. Copy binary_inspect addresses; subtract image_base from a link-time VA, never reinterpret file offsets or page numbers.",
            ReplayPolicy::ReadOnly,
            true,
            |services, agent, call, name, args| {
                handlers::handle_functions_search(&services.analysis, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Ghidra,
        ),
        spec::<args::StageTargetArgs>(
            "stage_target",
            "tool-stage-target",
            "Copy the run's SHA256-verified original target into the worker's immutable files area.",
            ReplayPolicy::WorkerIdempotent,
            true,
            |services, agent, call, name, args| {
                handlers::handle_stage_target(&services.execution, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Worker(&[
                Method::UploadBegin,
                Method::UploadWrite,
                Method::UploadCommit,
            ]),
        ),
        spec::<args::FilePutArgs>(
            "file_put",
            "tool-file-put",
            "Create an immutable worker file from base64 bytes (data_base64), UTF-8 text (text — preferred for scripts: no base64 encoding needed) or an existing evidence artifact. evidence_id must be the CONTENT evidence id — the record whose bytes you actually want (a tool result's content_evidence_id), NOT the tool-result envelope id: passing the envelope id writes the envelope JSON, not the bytes. Use executable=true when the file will be executed as a program. path_policy=versioned requires a logical filename (solve.py) and stores a content-hash version; reuse the returned files/... path. Use file_revise for small changes.",
            ReplayPolicy::WorkerIdempotent,
            true,
            |services, agent, call, name, args| {
                handlers::handle_file_put(&services.execution, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Worker(&[Method::FilePut]),
        ),
        spec::<args::FileGetArgs>(
            "file_get",
            "tool-file-get",
            "Retrieve a declared in-run worker file and persist original bytes as evidence.",
            ReplayPolicy::ReadOnly,
            true,
            |services, agent, call, name, args| {
                handlers::handle_file_get(&services.execution, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Worker(&[Method::FileStat, Method::FileRead]),
        ),
        spec::<args::FileReviseArgs>(
            "file_revise",
            "tool-file-revise",
            "Create an immutable UTF-8 revision from original content evidence and 1..64 sequential, uniquely matching text replacements. name is a logical filename; reuse the returned hash-bound worker path and content_evidence_id. Originals are never overwritten. Prefer small revisions over retransmitting a complete script.",
            ReplayPolicy::WorkerIdempotent,
            true,
            |services, agent, call, name, args| {
                handlers::handle_file_revise(&services.execution, agent, call, name, args)
            },
            Visibility::Child,
            Requirement::Worker(&[Method::FilePut]),
        ),
        spec::<args::ExecCommandArgs>(
            "exec_command",
            "tool-exec-command",
            "Run ONE bounded shell command synchronously in the worker and return its stdout/stderr/exit code in the result. The command runs in a fresh temporary directory: pass target=<staged path from stage_target> and the resolved file is available at $TARGET in the environment (\"printf '<payload>' | $TARGET\"). Prefer this for quick inspection and stdin-driven PoCs; long-running work must go to job_start with output redirected to a log file instead of blocking here. Worker paths (files/...) resolve only as direct argv entries, never inside shell strings; use $WUKONG_STATE_DIR for the state root.",
            ReplayPolicy::WorkerIdempotent,
            true,
            |services, agent, call, name, args| {
                handlers::handle_job_start(&services.execution, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Worker(&[Method::ExecRun]),
        ),
        spec::<args::JobStartArgs>(
            "job_start",
            "tool-job-start",
            "Start a persistent process job in the configured worker; returns a job_id immediately. Use argv, or a staged launch_spec with its recorded target, files, arguments, cwd, environment and input mode; input selects a staged data file. With launch_spec, omit argv/env/cwd overrides and keep its recorded timeout. Raw argv and executable_inputs may use {target}; files/... references resolve only as direct entries, never inside shell strings. On isolated Linux, argv[0] and explicitly listed executable_inputs get executable copies; other file arguments are read-only data. Execution results include cwd and input hashes/access. Jobs may read WUKONG_STATE_DIR and WUKONG_JOB_DIR from the environment.",
            ReplayPolicy::WorkerIdempotent,
            true,
            |services, agent, call, name, args| {
                handlers::handle_job_start(&services.execution, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Worker(&[Method::JobStart]),
        ),
        spec::<args::TraceStartArgs>(
            "trace_run",
            "tool-trace-run",
            "Start a declarative dynamic trace from an immutable LaunchSpec and same-run input evidence. Trace points must bind the LaunchSpec source target. Returns an ordinary owned job immediately; observe it with job_status and import run.json/events.jsonl with job_artifacts. No debugger session is created.",
            ReplayPolicy::WorkerIdempotent,
            true,
            |services, agent, call, name, args| {
                handlers::handle_job_start(&services.execution, agent, call, name, args)
            },
            Visibility::Child,
            Requirement::Worker(&[Method::TraceStart]),
        ),
        spec::<args::JobStatusArgs>(
            "job_status",
            "tool-job-status",
            "Observe an in-run job, refresh persisted status and import raw observations.",
            ReplayPolicy::ReadOnly,
            true,
            |services, agent, call, name, args| {
                handlers::handle_job_start(&services.execution, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Worker(&[Method::JobStatus]),
        ),
        spec::<args::JobCancelArgs>(
            "job_cancel",
            "tool-job-cancel",
            "Request cancellation of an owned in-run job.",
            ReplayPolicy::WorkerIdempotent,
            true,
            |services, agent, call, name, args| {
                handlers::handle_job_start(&services.execution, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Worker(&[Method::JobCancel]),
        ),
        spec::<args::JobArtifactsArgs>(
            "job_artifacts",
            "tool-job-artifacts",
            "List and import original artifacts produced by an in-run job.",
            ReplayPolicy::ReadOnly,
            true,
            |services, agent, call, name, args| {
                handlers::handle_job_start(&services.execution, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Worker(&[Method::JobArtifacts]),
        ),
        spec::<args::FuzzStartArgs>(
            "fuzz_start",
            "tool-fuzz-start",
            "Start AFL++ QEMU fuzzing after staging target and nonempty seed files.",
            ReplayPolicy::WorkerIdempotent,
            true,
            |services, agent, call, name, args| {
                handlers::handle_job_start(&services.execution, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Worker(&[Method::FuzzStart]),
        ),
        spec::<args::DebugStartArgs>(
            "debug_start",
            "tool-debug-start",
            "Start an owned debugger session on the staged target, or inspect a separately staged dump. Engine selection never falls back to another platform.",
            ReplayPolicy::WorkerIdempotent,
            true,
            |services, agent, call, name, args| {
                handlers::handle_job_start(&services.execution, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Worker(&[Method::DebugStart]),
        ),
        spec::<args::DebugCommandArgs>(
            "debug_command",
            "tool-debug-command",
            "Execute one policy-checked GDB/MI command in your owned session. Isolated sessions accept simple noncalling expressions; prefer typed inspection/control. Stable request IDs prevent blind replay.",
            ReplayPolicy::WorkerIdempotent,
            true,
            |services, agent, call, name, args| {
                handlers::handle_job_start(&services.execution, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Worker(&[Method::DebugCommand]),
        ),
        spec::<args::DebugInspectArgs>(
            "debug_inspect",
            "tool-debug-inspect",
            "Inspect bounded registers, stack, memory, modules, threads, instructions, exception or execution state in your owned debugger session. Addresses are runtime virtual addresses in that session. kind=events requires debug.events support and accepts offset/limit; full engine transcripts remain evidence.",
            ReplayPolicy::ReadOnly,
            true,
            |services, agent, call, name, args| {
                handlers::handle_job_start(&services.execution, agent, call, name, args)
            },
            Visibility::Child,
            Requirement::Worker(&[Method::DebugInspect]),
        ),
        spec::<args::DebugControlArgs>(
            "debug_control",
            "tool-debug-control",
            "Control your owned live debugger using typed breakpoint and execution actions. Dump sessions reject execution control; stable request IDs prevent blind replay.",
            ReplayPolicy::WorkerIdempotent,
            true,
            |services, agent, call, name, args| {
                handlers::handle_job_start(&services.execution, agent, call, name, args)
            },
            Visibility::Child,
            Requirement::Worker(&[Method::DebugControl]),
        ),
        spec::<args::DebugCloseArgs>(
            "debug_close",
            "tool-debug-close",
            "Close your debugger session, or clean up an in-run session whose owner is terminal.",
            ReplayPolicy::WorkerIdempotent,
            true,
            |services, agent, call, name, args| {
                handlers::handle_job_start(&services.execution, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Worker(&[Method::DebugClose]),
        ),
        spec::<args::InteractiveStartArgs>(
            "interactive_start",
            "tool-interactive-start",
            "Start a persistent interactive process (REPL-style program) owned by this agent; returns a session_id. Worker paths resolve as direct argv entries.",
            ReplayPolicy::WorkerIdempotent,
            true,
            |services, agent, call, name, args| {
                handlers::handle_job_start(&services.execution, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Worker(&[Method::InteractStart]),
        ),
        spec::<args::InteractiveSendArgs>(
            "interactive_send",
            "tool-interactive-send",
            "Write at most 64 KiB to an owned session using source={kind:text,text:...}, {kind:base64,data_base64:...} or {kind:evidence,evidence_id:...}. Returns newly produced output; the session stays alive between calls.",
            ReplayPolicy::WorkerIdempotent,
            true,
            |services, agent, call, name, args| {
                handlers::handle_job_start(&services.execution, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Worker(&[Method::InteractWrite]),
        ),
        spec::<args::InteractiveCloseArgs>(
            "interactive_close",
            "tool-interactive-close",
            "Close your interactive session and terminate its process tree; also permits cleanup when its in-run owner is terminal.",
            ReplayPolicy::WorkerIdempotent,
            true,
            |services, agent, call, name, args| {
                handlers::handle_job_start(&services.execution, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Worker(&[Method::InteractClose]),
        ),
        spec::<args::AngrSolveArgs>(
            "angr_solve",
            "tool-angr-solve",
            "Bounded symbolic input reconstruction for stdin validation paths. Supply module-relative success/failure addresses and expected input length. Returns a background job with candidate input and exploration statistics. Prefer direct inversion for simple reversible transforms; slice or use explicit constraints for large state spaces. Always replay candidate bytes on the real target.",
            ReplayPolicy::WorkerIdempotent,
            true,
            |services, agent, call, name, args| {
                handlers::handle_job_start(&services.execution, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Worker(&[Method::AngrSolve]),
        ),
        spec::<args::PyRunArgs>(
            "py_run",
            "tool-py-run",
            "Run a Python script in the worker in ONE step. Pass small scripts as source text; for larger scripts use file_put with text and pass the returned path as source_file (exactly one of source/source_file). The worker syntax-checks it and returns the compiler error inline before spending a job; then runs it as a background job (job_status for stdout/stderr). Optional argv entries arrive as sys.argv[1..]; an optional target=<staged path> is appended LAST to argv as a read-only executable copy in isolated jobs — read it from sys.argv[-1].",
            ReplayPolicy::WorkerIdempotent,
            true,
            |services, agent, call, name, args| {
                handlers::handle_job_start(&services.execution, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Worker(&[Method::PythonRun]),
        ),
        spec::<args::LibcIdentifyArgs>(
            "libc_identify",
            "tool-libc-identify",
            "Compare same-process symbol leaks against up to 16 supplied in-run ELF libc candidates, or only the worker's libc when omitted. Preserve candidate hashes, Build IDs, architecture, inconsistent leaks and ambiguous matches. Candidates never confirm the mapped runtime; no match describes only the searched corpus.",
            ReplayPolicy::ReadOnly,
            true,
            |services, agent, call, name, args| {
                handlers::handle_libc_identify(&services.execution, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Worker(&[Method::LibcIdentify]),
        ),
        spec::<args::EmulateRunArgs>(
            "emulate_run",
            "tool-emulate-run",
            "Bounded Unicorn emulation of the staged target from a start address: maps the ELF segments plus a stack, executes up to count instructions or until the PC reaches the until address (whichever first), and returns registers plus requested memory dumps (data_base64). The target never runs natively — this is a pure simulation for VM-style or hand-crafted code (interpreter loops, x87/FPU blocks) without hand-building Unicorn harnesses. Use init_regs for register preconditions and memory to inject bytes. The watchdog timeout bounds runaway loops; unsupported instructions report the failing address in error.",
            ReplayPolicy::ReadOnly,
            true,
            |services, agent, call, name, args| {
                handlers::handle_emulate_run(&services.execution, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Worker(&[Method::EmulateRun]),
        ),
        spec::<args::CrashReplayArgs>(
            "crash_replay",
            "tool-crash-replay",
            "Start a persistent clean-directory crash reproduction job.",
            ReplayPolicy::WorkerIdempotent,
            true,
            |services, agent, call, name, args| {
                handlers::handle_job_start(&services.execution, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Worker(&[Method::CrashReplay]),
        ),
        spec::<args::InputMinimizeArgs>(
            "input_minimize",
            "tool-input-minimize",
            "Minimize a crashing input with fresh module-hash, PC-offset and frame signatures using the worker's debugger (GDB on Linux, DbgEng through WinDbg MCP on Windows). allow_signal_only explicitly permits weaker signal-only equivalence. Poll job_status and collect job_artifacts.",
            ReplayPolicy::WorkerIdempotent,
            true,
            |services, agent, call, name, args| {
                handlers::handle_job_start(&services.execution, agent, call, name, args)
            },
            Visibility::All,
            Requirement::Worker(&[Method::InputMinimize]),
        ),
        spec::<args::BundleEmptyArgs>(
            "bundle_manifest",
            "tool-bundle-manifest",
            "Return the directory bundle manifest: auto-hashed members, inferred executable/library/data kinds, PE imports and cross-member import edges. No user-assigned roles; entry is the inferred launch candidate.",
            ReplayPolicy::ReadOnly,
            true,
            |services, agent, call, name, args| {
                handlers::handle_bundle_manifest(&services.bundle, agent, call, name, args)
            },
            Visibility::Child,
            Requirement::Controller,
        ),
        spec::<args::BundleMemberArgs>(
            "bundle_evidence",
            "tool-bundle-evidence",
            "Import one verified bundle member as immutable evidence so it can be inspected or decompiled with target_ref={kind:evidence,evidence_id:...}. Path is relative to the analyzed directory.",
            ReplayPolicy::ReadOnly,
            true,
            |services, agent, call, name, args| {
                handlers::handle_bundle_evidence(&services.bundle, agent, call, name, args)
            },
            Visibility::Child,
            Requirement::Controller,
        ),
        spec::<args::PocVerifyArgs>(
            "poc_verify",
            "tool-poc-verify",
            "Independently replay a finding in fresh trial directories. Use argv/stdin delivery, or a bounded declarative network plan with the host-prepared launch_spec. Network trials use fresh TCP connections, capture/patch ASLR-dependent values and hash raw transcripts; server state is not reset and deployed runtime identity remains unconfirmed. Native Windows workers support argv/stdin replay only. Worker observations determine success.",
            ReplayPolicy::WorkerIdempotent,
            true,
            |services, agent, call, name, args| {
                handlers::handle_job_start(&services.execution, agent, call, name, args)
            },
            Visibility::Verifier,
            Requirement::Worker(&[Method::PocVerify]),
        ),
        spec::<args::BundleEmptyArgs>(
            "lab_list",
            "tool-lab-list",
            "List configured Windows lab VM aliases and assigned environments without connecting or changing state.",
            ReplayPolicy::ReadOnly,
            false,
            |services, agent, call, name, args| {
                handlers::handle_lab_list(&services.lab, agent, call, name, args)
            },
            Visibility::Child,
            Requirement::Controller,
        ),
        spec::<args::LabOperationArgs>(
            "lab_operation_status",
            "tool-lab-operation-status",
            "Read a durable lab host operation by operation_id, including its result and evidence. A lost controller leaves the operation uncertain and never replays it.",
            ReplayPolicy::ReadOnly,
            false,
            |services, agent, call, name, args| {
                handlers::handle_lab_operation(&services.lab, agent, call, name, args)
            },
            Visibility::Child,
            Requirement::Controller,
        ),
        spec::<args::LabOperationArgs>(
            "lab_operation_cancel",
            "tool-lab-operation-cancel",
            "Request interruption of your lab host operation. Cancellation can leave guest effects unknown; inspect and reconcile the existing operation before another mutation.",
            ReplayPolicy::Transactional,
            true,
            |services, agent, call, name, args| {
                handlers::handle_lab_operation(&services.lab, agent, call, name, args)
            },
            Visibility::Child,
            Requirement::Controller,
        ),
        spec::<args::LabStatusArgs>(
            "lab_status",
            "tool-lab-status",
            "Read the persisted state and bounded host observations for one registered Windows lab VM. Requires vm; use lab_list for discovery. Does not reconcile or modify state and remains available during long operations.",
            ReplayPolicy::ReadOnly,
            false,
            |services, agent, call, name, args| {
                handlers::handle_lab_status(&services.lab, agent, call, name, args)
            },
            Visibility::Child,
            Requirement::Controller,
        ),
        spec::<args::LabPrepareArgs>(
            "lab_prepare",
            "tool-lab-prepare",
            "Start preparation of the assigned Windows lab and return operation_id. Every attempt restores fixed base wukong_base_vm; configured signed worker_media may upgrade, verify and republish that base before a reboot and experiment snapshot. Query lab_operation_status for the owned lab_ref and lease. The controller derives ownership and package hashes from assessment evidence; spec is typed. Available by default to catalog-permitted roles. Unsupported execution requirements are rejected at run admission.",
            ReplayPolicy::OperatorOnly,
            true,
            |services, agent, call, name, args| {
                handlers::handle_lab_prepare(&services.lab, agent, call, name, args)
            },
            Visibility::Child,
            Requirement::Controller,
        ),
        spec::<args::LabRunArgs>(
            "lab_run",
            "tool-lab-run",
            "Start a verified exe/user harness using the owned lab_ref as an ordinary durable assessment job and return run.job_id. Use job_status, job_cancel and job_artifacts with that handle. Supports stdin evidence or empty argv; declared target hash must match. Shared assessment leases and completion gates remain in force.",
            ReplayPolicy::OperatorOnly,
            true,
            |services, agent, call, name, args| {
                handlers::handle_lab_run(&services.lab, agent, call, name, args)
            },
            Visibility::Child,
            Requirement::Worker(&[
                Method::JobStart,
                Method::FilePut,
                Method::JobStatus,
                Method::JobCancel,
            ]),
        ),
        spec::<args::LabInstallArgs>(
            "lab_install",
            "tool-lab-install",
            "Start installation using the owned lab_ref and return operation_id. Uses one overall timeout (1-600 seconds), hash-pinned media and a guest receipt checking declared files, service and account. Query lab_operation_status; missing receipts remain uncertain. Requires idle worker resources and synchronized evidence.",
            ReplayPolicy::OperatorOnly,
            true,
            |services, agent, call, name, args| {
                handlers::handle_lab_install(&services.lab, agent, call, name, args)
            },
            Visibility::Child,
            Requirement::Controller,
        ),
        spec::<args::LabRestoreArgs>(
            "lab_restore",
            "tool-lab-restore",
            "Start a fenced baseline restore using the owned lab_ref and return operation_id. Requires idle worker resources and synchronized evidence. Unknown effects become lost only after confirmed restoration; snapshot pruning is reported separately. Query lab_operation_status.",
            ReplayPolicy::OperatorOnly,
            true,
            |services, agent, call, name, args| {
                handlers::handle_lab_restore(&services.lab, agent, call, name, args)
            },
            Visibility::Child,
            Requirement::Controller,
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
        let validator = builtins()
            .iter()
            .find(|s| s.definition.name == "artifact_validate")
            .unwrap();
        assert!(!validator.allows(Role::Root));
        assert!(!validator.allows(Role::Analyst));
        assert!(validator.allows(Role::Verifier));
        for name in ["lab_status", "lab_prepare", "lab_run", "lab_install"] {
            let spec = builtins()
                .iter()
                .find(|s| s.definition.name.as_str() == name)
                .unwrap();
            assert!(!spec.allows(Role::Root), "{name}");
            assert!(spec.allows(Role::Analyst), "{name}");
            assert!(spec.allows(Role::Verifier), "{name}");
        }
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
