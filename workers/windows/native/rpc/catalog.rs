//! One registry owns public method routing and advertised Windows capabilities.
use serde_json::{Value, json};
use wukong_worker_protocol::{Method, WorkerCapacity, WorkerIdentity};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Route {
    Files,
    Execution,
    Analysis,
}

#[derive(Clone, Copy)]
pub(crate) enum Handler {
    Files,
    Jobs,
    Session,
    Launch(crate::executors::plan::Factory),
}
pub(crate) struct Registration {
    pub method: Method,
    pub handler: Handler,
}
const fn registered(method: Method, handler: Handler) -> Registration {
    Registration { method, handler }
}

pub(crate) struct Group {
    pub route: Route,
    pub capabilities: &'static [&'static str],
    pub methods: &'static [Registration],
}

pub(crate) const GROUPS: &[Group] = &[
    Group {
        route: Route::Files,
        capabilities: &["resumable_file_transfer"],
        methods: &[
            registered(Method::FilePut, Handler::Files),
            registered(Method::FileGet, Handler::Files),
            registered(Method::FileStat, Handler::Files),
            registered(Method::FileRead, Handler::Files),
            registered(Method::UploadBegin, Handler::Files),
            registered(Method::UploadWrite, Handler::Files),
            registered(Method::UploadStatus, Handler::Files),
            registered(Method::UploadCommit, Handler::Files),
        ],
    },
    Group {
        route: Route::Execution,
        capabilities: &[
            "process_jobs",
            "process_stdin",
            "launch_spec_jobs",
            "persistent_jobs",
            "native_target_execution",
        ],
        methods: &[
            registered(
                Method::JobStart,
                Handler::Launch(crate::executors::plan::process),
            ),
            registered(Method::JobStatus, Handler::Jobs),
            registered(Method::JobCancel, Handler::Jobs),
            registered(Method::JobArtifacts, Handler::Jobs),
            registered(Method::JobList, Handler::Jobs),
            registered(Method::ResourceList, Handler::Jobs),
        ],
    },
    Group {
        route: Route::Execution,
        capabilities: &["debug_dbgeng"],
        methods: &[
            registered(
                Method::DebugStart,
                Handler::Launch(crate::executors::session_plan::debugger),
            ),
            registered(Method::DebugInspect, Handler::Session),
            registered(Method::DebugProfile, Handler::Session),
            registered(Method::DebugControl, Handler::Session),
            registered(Method::DebugClose, Handler::Session),
        ],
    },
    Group {
        route: Route::Execution,
        capabilities: &["interactive_process"],
        methods: &[
            registered(
                Method::InteractStart,
                Handler::Launch(crate::executors::session_plan::interactive),
            ),
            registered(Method::InteractWrite, Handler::Session),
            registered(Method::InteractProfile, Handler::Session),
            registered(Method::InteractClose, Handler::Session),
        ],
    },
    Group {
        route: Route::Execution,
        capabilities: &["shell_exec"],
        methods: &[registered(
            Method::ExecRun,
            Handler::Launch(crate::executors::shell::plan),
        )],
    },
    Group {
        route: Route::Execution,
        capabilities: &["py_run"],
        methods: &[registered(
            Method::PythonRun,
            Handler::Launch(crate::executors::python::plan),
        )],
    },
    Group {
        route: Route::Execution,
        capabilities: &["crash_replay"],
        methods: &[registered(
            Method::CrashReplay,
            Handler::Launch(crate::executors::replay_plan::replay),
        )],
    },
    Group {
        route: Route::Execution,
        capabilities: &["input_minimize"],
        methods: &[registered(
            Method::InputMinimize,
            Handler::Launch(crate::executors::replay_plan::minimize),
        )],
    },
    Group {
        route: Route::Execution,
        capabilities: &["poc_verify"],
        methods: &[registered(
            Method::PocVerify,
            Handler::Launch(crate::executors::replay_plan::verify),
        )],
    },
    Group {
        route: Route::Files,
        capabilities: &["runtime_profile", "mitigation_profile"],
        methods: &[registered(Method::RuntimeProfile, Handler::Files)],
    },
    Group {
        route: Route::Analysis,
        capabilities: &["seccomp_analyze"],
        methods: &[registered(
            Method::SeccompAnalyze,
            Handler::Launch(crate::executors::analysis::plan),
        )],
    },
];

pub(crate) fn route(method: &str) -> Option<Route> {
    GROUPS
        .iter()
        .find(|group| {
            group
                .methods
                .iter()
                .any(|registered| registered.method.name() == method)
        })
        .map(|group| group.route)
}

pub(crate) fn doctor(identity: &WorkerIdentity, qualified: bool) -> Value {
    let mut capabilities = serde_json::Map::new();
    for group in GROUPS {
        for name in group.capabilities {
            capabilities.insert(
                (*name).into(),
                Value::Bool(group.route == Route::Files || qualified),
            );
        }
    }
    capabilities.insert("formats".into(), json!(["PE"]));
    json!({"protocol":identity.protocol,"identity":identity,"os":"windows",
        "isolation":"appcontainer","metadata_protected_from_targets":true,
        "exclusive_run_lease":true,"capabilities":capabilities,
        "methods":GROUPS.iter().flat_map(|group| group.methods.iter().map(move |entry| (entry.method.name().to_owned(), json!({"ready":entry.method.resources() == wukong_worker_protocol::ResourceDemand::None || qualified})))).collect::<serde_json::Map<String,Value>>(),
        "capacity":WorkerCapacity{max_active_resources:1,exclusive_run_lease:true},
        "readiness":{"ready":qualified,"reason":if qualified {"current service isolation and toolchain qualification passed"} else {"startup qualification incomplete"}},
        "implementation":"native_windows","execution_owner":"native_windows",
        "analysis_runtime":"isolated_python_process","production_execution":qualified})
}

pub(crate) fn handler(method: Method) -> Option<Handler> {
    GROUPS
        .iter()
        .flat_map(|group| group.methods)
        .find(|entry| entry.method == method)
        .map(|entry| entry.handler)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registry_has_unique_methods_and_never_enables_execution_before_qualification() {
        let mut names = std::collections::BTreeSet::new();
        let identity = WorkerIdentity {
            protocol: 3,
            installation_id: "a".repeat(32),
            boot_id: "fixture".into(),
            package_sha256: "b".repeat(64),
        };
        for group in GROUPS {
            for entry in group.methods {
                let method = entry.method;
                assert!(names.insert(method), "duplicate {method:?}");
            }
            for capability in group.capabilities {
                assert_eq!(
                    doctor(&identity, false)["capabilities"][capability],
                    group.route == Route::Files
                );
                assert_eq!(doctor(&identity, true)["capabilities"][capability], true);
            }
        }
        assert!(route("debug.command").is_none());
        assert!(route("worker.shutdown").is_none());
        for method in [
            Method::LibcIdentify,
            Method::CyclicOffset,
            Method::EmulateRun,
        ] {
            assert!(
                route(method.name()).is_none(),
                "offline package lacks this tool's dependencies"
            );
        }
    }
}
