//! One registry owns public method routing and advertised Windows capabilities.
use serde_json::{Value, json};
use wukong_worker_protocol::{Method, WorkerCapacity, WorkerIdentity};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Route {
    Files,
    Execution,
    Analysis,
}

pub(crate) struct Group {
    pub route: Route,
    pub capabilities: &'static [&'static str],
    pub methods: &'static [Method],
}

pub(crate) const GROUPS: &[Group] = &[
    Group {
        route: Route::Files,
        capabilities: &["resumable_file_transfer"],
        methods: &[
            Method::FilePut,
            Method::FileGet,
            Method::FileStat,
            Method::FileRead,
            Method::UploadBegin,
            Method::UploadWrite,
            Method::UploadStatus,
            Method::UploadCommit,
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
            Method::JobStart,
            Method::JobStatus,
            Method::JobCancel,
            Method::JobArtifacts,
            Method::JobList,
            Method::ResourceList,
        ],
    },
    Group {
        route: Route::Execution,
        capabilities: &["debug_dbgeng"],
        methods: &[
            Method::DebugStart,
            Method::DebugInspect,
            Method::DebugProfile,
            Method::DebugControl,
            Method::DebugClose,
        ],
    },
    Group {
        route: Route::Execution,
        capabilities: &["interactive_process"],
        methods: &[
            Method::InteractStart,
            Method::InteractWrite,
            Method::InteractProfile,
            Method::InteractClose,
        ],
    },
    Group {
        route: Route::Execution,
        capabilities: &["shell_exec"],
        methods: &[Method::ExecRun],
    },
    Group {
        route: Route::Execution,
        capabilities: &["py_run"],
        methods: &[Method::PythonRun],
    },
    Group {
        route: Route::Execution,
        capabilities: &["crash_replay"],
        methods: &[Method::CrashReplay],
    },
    Group {
        route: Route::Execution,
        capabilities: &["input_minimize"],
        methods: &[Method::InputMinimize],
    },
    Group {
        route: Route::Execution,
        capabilities: &["poc_verify"],
        methods: &[Method::PocVerify],
    },
    Group {
        route: Route::Files,
        capabilities: &["runtime_profile", "mitigation_profile"],
        methods: &[Method::RuntimeProfile],
    },
    Group {
        route: Route::Analysis,
        capabilities: &["seccomp_analyze"],
        methods: &[Method::SeccompAnalyze],
    },
];

pub(crate) fn route(method: &str) -> Option<Route> {
    GROUPS
        .iter()
        .find(|group| {
            group
                .methods
                .iter()
                .any(|registered| registered.name() == method)
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
        "methods":GROUPS.iter().flat_map(|group| group.methods.iter().map(move |method| (method.name().to_owned(), json!({"ready":method.resources() == wukong_worker_protocol::ResourceDemand::None || qualified})))).collect::<serde_json::Map<String,Value>>(),
        "capacity":WorkerCapacity{max_active_resources:1,exclusive_run_lease:true},
        "readiness":{"ready":qualified,"reason":if qualified {"current service isolation and toolchain qualification passed"} else {"startup qualification incomplete"}},
        "implementation":"native_windows","execution_owner":"native_windows",
        "analysis_runtime":"isolated_python_process","production_execution":qualified})
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
            for method in group.methods {
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
