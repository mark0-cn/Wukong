//! Backend requirements live beside schemas and apply to selection and dispatch.
use super::*;
use wukong_worker_protocol::Method;

#[derive(Clone, Copy, Debug)]
pub(super) enum Requirement {
    Controller,
    Probe,
    Ghidra,
    Worker(&'static [Method]),
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
    pub(super) fn available(self, observation: Option<&Value>, local_ghidra: bool) -> bool {
        match self {
            Self::Controller | Self::Probe => true,
            Self::Ghidra => {
                local_ghidra
                    || observation.is_some_and(|value| {
                        value["methods"][Method::GhidraQuery.name()]["ready"] == true
                    })
            }
            Self::Worker(methods) => observation.is_some_and(|value| {
                methods
                    .iter()
                    .all(|method| value["methods"][method.name()]["ready"] == true)
            }),
        }
    }

    pub(super) fn label(self) -> String {
        match self {
            Self::Controller => "controller".into(),
            Self::Probe => "explicit probe".into(),
            Self::Ghidra => "controller Ghidra or ghidra_query".into(),
            Self::Worker(methods) => methods
                .iter()
                .map(|m| m.name())
                .collect::<Vec<_>>()
                .join(" + "),
        }
    }
}

impl ToolService {
    pub(super) fn local_ghidra(&self) -> Option<PathBuf> {
        self.analysis_config
            .ghidra_executable
            .clone()
            .or_else(|| std::env::var_os("WUKONG_GHIDRA_HEADLESS").map(PathBuf::from))
    }

    pub(super) fn needs_worker(&self, spec: &catalog::Spec) -> bool {
        match spec.requirement {
            Requirement::Controller | Requirement::Probe => false,
            Requirement::Ghidra => self.local_ghidra().is_none(),
            Requirement::Worker(_) => true,
        }
    }
}

impl ToolContext<'_> {
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
    use wukong_worker_protocol::Method;
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
        let doctor = json!({"os":"windows","capabilities":{"debug_dbgeng":true,"debug_gdb_mi":true,"fuzz_qemu":true,"resumable_file_transfer":true},"methods":{"debug.start":{"ready":true},"debug.control":{"ready":true},"file.put":{"ready":true}}});
        for name in ["debug_start", "debug_control", "file_put"] {
            assert!(
                catalog::builtins()
                    .iter()
                    .find(|spec| spec.definition.name.as_str() == name)
                    .unwrap()
                    .requirement
                    .available(Some(&doctor), false),
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
                !catalog::builtins()
                    .iter()
                    .find(|spec| spec.definition.name.as_str() == name)
                    .unwrap()
                    .requirement
                    .available(Some(&doctor), false),
                "{name}"
            );
        }
        assert!(!Requirement::Worker(&[Method::DebugStart]).available(None, false));
        assert!(Requirement::Controller.available(None, false));
    }
}
