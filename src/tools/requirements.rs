//! Backend requirements live beside schemas and apply to selection and dispatch.
use super::*;

#[derive(Clone, Copy, Debug)]
pub(super) enum Requirement {
    Controller,
    Probe,
    Ghidra,
    Worker {
        linux: bool,
        any: &'static [&'static str],
    },
}

impl Requirement {
    fn job_inputs_available(args: &Value, observation: Option<&Value>) -> bool {
        let capability = if args["launch_spec"].is_string() {
            Some("launch_spec_jobs")
        } else if args["input"].is_string() {
            Some("process_stdin")
        } else {
            None
        };
        capability.is_none_or(|capability| {
            observation.is_some_and(|value| value["capabilities"][capability] == true)
        })
    }
    pub(super) fn for_tool(name: &str) -> Self {
        use Requirement::*;
        let (linux, any): (bool, &'static [&'static str]) = match name {
            "runtime_doctor" | "environment_doctor" => return Probe,
            "functions_search" | "function_decompile" | "function_instructions" | "xrefs_query" => {
                return Ghidra;
            }
            "artifact_stage" | "stage_target" | "file_put" | "file_get" => {
                (false, &["resumable_file_transfer"])
            }
            "job_start" | "differential_test" => (false, &["process_jobs"]),
            "job_status" | "job_cancel" | "job_artifacts" | "fuzz_status" | "debug_close"
            | "interactive_close" => (false, &[]),
            "debug_start" | "debug_inspect" | "debug_control" => {
                (false, &["debug_gdb_mi", "debug_dbgeng"])
            }
            "debug_command" => (true, &["debug_gdb_mi"]),
            "interactive_start" | "interactive_send" => (false, &["interactive_process"]),
            "exec_command" => (false, &["shell_exec"]),
            "py_run" => (false, &["py_run"]),
            "fuzz_start" => (true, &["fuzz_qemu"]),
            "crash_replay" => (false, &["crash_replay"]),
            "input_minimize" => (false, &["input_minimize", "input_minimize_signal_only"]),
            "poc_verify" | "verification_record" => (false, &["poc_verify"]),
            "angr_solve" => (false, &["symbolic_execution"]),
            "emulate_run" => (false, &["emulation"]),
            "libc_identify" => (false, &["libc_identify"]),
            "seccomp_analyze" => (false, &["seccomp_analyze"]),
            "runtime_profile" => (false, &["runtime_profile"]),
            "runtime_prepare" => (false, &["runtime_prepare"]),
            "mitigation_profile" => (false, &["mitigation_profile"]),
            "crash_snapshot" => (true, &["core_snapshot"]),
            "cyclic_offset" => (false, &["cyclic_offset"]),
            "environment_list"
            | "analysis_verify"
            | "create_agent"
            | "view_agent_graph"
            | "send_message_to_agent"
            | "follow_up_agent"
            | "message_status"
            | "wait_for_agents"
            | "stop_agent"
            | "agent_finish"
            | "finish_scan"
            | "threat_model_get"
            | "threat_model_save"
            | "threat_model_amend"
            | "note_save"
            | "notes_list"
            | "context_search"
            | "context_read"
            | "todo_save"
            | "todo_update"
            | "todos_list"
            | "coverage_record"
            | "coverage_update"
            | "coverage_list"
            | "finding_file"
            | "finding_update"
            | "findings_list"
            | "finding_get"
            | "evidence_read"
            | "load_skill"
            | "think"
            | "binary_inspect"
            | "binary_strings" => return Controller,
            _ => panic!("tool has no registered backend requirement: {name}"),
        };
        Worker { linux, any }
    }

    pub(super) fn available(self, observation: Option<&Value>, local_ghidra: bool) -> bool {
        match self {
            Self::Controller | Self::Probe => true,
            Self::Ghidra => {
                local_ghidra
                    || observation
                        .is_some_and(|value| value["capabilities"]["ghidra_query"] == true)
            }
            Self::Worker { linux, any } => observation.is_some_and(|value| {
                (!linux || value["os"] == "linux")
                    && (any.is_empty()
                        || any
                            .iter()
                            .any(|capability| value["capabilities"][capability] == true))
            }),
        }
    }

    pub(super) fn label(self) -> String {
        match self {
            Self::Controller => "controller".into(),
            Self::Probe => "explicit probe".into(),
            Self::Ghidra => "controller Ghidra or ghidra_query".into(),
            Self::Worker { any: [], .. } => "worker transport".into(),
            Self::Worker { linux, any } => {
                format!("{}{}", if linux { "linux: " } else { "" }, any.join(" / "))
            }
        }
    }
}

impl ToolService {
    pub(super) fn local_ghidra(&self) -> Option<PathBuf> {
        self.analysis
            .config
            .ghidra_executable
            .clone()
            .or_else(|| std::env::var_os("WUKONG_GHIDRA_HEADLESS").map(PathBuf::from))
    }

    pub(super) fn needs_worker(&self, spec: &catalog::Spec) -> bool {
        match spec.requirement {
            Requirement::Controller | Requirement::Probe => false,
            Requirement::Ghidra => self.local_ghidra().is_none(),
            Requirement::Worker { .. } => true,
        }
    }

    pub(super) async fn require_backend(&self, spec: &catalog::Spec, args: &Value) -> Result<()> {
        let observation =
            if self.needs_worker(spec) && !matches!(spec.requirement, Requirement::Probe) {
                Some(self.execution.gateway.runtime()?.doctor().await?)
            } else {
                None
            };
        ensure!(
            spec.requirement.available(
                observation.as_ref(),
                self.local_ghidra().is_some_and(|path| path.is_file())
            ),
            "tool {} requires an observed backend capability: {}",
            spec.definition.name,
            spec.requirement.label()
        );
        if spec.definition.name.as_str() == "debug_start" {
            let observation = observation.as_ref().context("debugger preflight missing")?;
            let capability = match args["engine"].as_str().unwrap_or("auto") {
                "gdb_mi" => "debug_gdb_mi",
                "dbgeng" => "debug_dbgeng",
                _ if observation["os"] == "windows" => "debug_dbgeng",
                _ => "debug_gdb_mi",
            };
            ensure!(
                observation["capabilities"][capability] == true,
                "requested debugger capability is unavailable: {capability}"
            );
        }
        if spec.definition.name.as_str() == "job_start" {
            ensure!(
                Requirement::job_inputs_available(args, observation.as_ref()),
                "requested job LaunchSpec/stdin capability is unavailable"
            );
        }
        if spec.definition.name.as_str() == "input_minimize" && args["allow_signal_only"] != true {
            ensure!(
                observation
                    .as_ref()
                    .is_some_and(|value| value["capabilities"]["input_minimize"] == true),
                "location-preserving minimization requires input_minimize capability"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn new_job_inputs_require_observed_support_without_hiding_basic_jobs() {
        let old = json!({"os":"linux","capabilities":{"process_jobs":true}});
        assert!(Requirement::job_inputs_available(
            &json!({"argv":["files/target"]}),
            Some(&old)
        ));
        for args in [
            json!({"launch_spec":"files/launch.json"}),
            json!({"argv":["files/target"],"input":"files/input"}),
        ] {
            assert!(!Requirement::job_inputs_available(&args, Some(&old)));
            assert!(!Requirement::job_inputs_available(&args, None));
            let ready = json!({"os":"linux","capabilities":{"process_jobs":true,"launch_spec_jobs":true,"process_stdin":true}});
            assert!(Requirement::job_inputs_available(&args, Some(&ready)));
        }
    }

    #[test]
    fn windows_debugger_never_enables_linux_tools_or_unobserved_execution() {
        let doctor = json!({"os":"windows","capabilities":{"debug_dbgeng":true,"debug_gdb_mi":true,"fuzz_qemu":true,"resumable_file_transfer":true}});
        for name in ["debug_start", "debug_control", "file_put"] {
            assert!(
                Requirement::for_tool(name).available(Some(&doctor), false),
                "{name}"
            );
        }
        for name in [
            "debug_command",
            "fuzz_start",
            "runtime_prepare",
            "poc_verify",
            "job_start",
            "interactive_start",
        ] {
            assert!(
                !Requirement::for_tool(name).available(Some(&doctor), false),
                "{name}"
            );
        }
        assert!(!Requirement::for_tool("debug_start").available(None, false));
        assert!(Requirement::for_tool("binary_inspect").available(None, false));
    }
}
