//! Debugger and interactive startup factories; resource ownership stays in the manager.
use crate::{
    artifacts::resource_input::Purpose,
    executors::{
        parameters::{clean, request_id, start_parameters},
        plan::Plan,
        specification::{Specification, default_cwd, default_output},
    },
    resources::models::ResourceKind,
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
fn auto() -> String {
    "auto".into()
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Start {
    pub(crate) mode: String,
    #[serde(default = "auto")]
    pub(crate) engine: String,
    pub(crate) target: Option<String>,
    #[serde(default)]
    pub(crate) args: Vec<String>,
    pub(crate) dump: Option<String>,
    pub(crate) input: Option<String>,
    pub(crate) launch_spec: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InteractiveStart {
    pub(crate) argv: Vec<String>,
    #[serde(default)]
    pub(crate) env: BTreeMap<String, String>,
    pub(crate) input: Option<String>,
    pub(crate) launch_spec: Option<String>,
}

pub(crate) fn debugger(params: &Map<String, Value>) -> Result<Plan> {
    let request_id = request_id(params)?;
    let start: Start = serde_json::from_value(Value::Object(clean(params)))?;
    ensure!(
        matches!(start.mode.as_str(), "launch" | "dump")
            && matches!(start.engine.as_str(), "auto" | "dbgeng")
            && (start.mode == "dump") == start.dump.is_some(),
        "native session requires launch or dump with auto/dbgeng; only dump mode requires a dump"
    );
    ensure!(
        start.launch_spec.is_some() || start.target.is_some(),
        "native session requires a target or LaunchSpec"
    );
    ensure!(
        start.launch_spec.is_none() || start.target.is_none() && start.args.is_empty(),
        "LaunchSpec owns debugger target and arguments"
    );
    ensure!(
        start.mode != "dump" || start.input.is_none() && start.args.is_empty(),
        "dump inspection cannot supply execution arguments or stdin"
    );
    let spec = Specification {
        argv: start
            .target
            .iter()
            .cloned()
            .chain(start.args.iter().cloned())
            .collect(),
        launch_spec: start.launch_spec.clone(),
        input: start.input.clone(),
        env: BTreeMap::new(),
        cwd: default_cwd(),
        timeout_secs: start.launch_spec.is_none().then_some(3600),
        output_limit: default_output(),
    };
    spec.validate()?;
    let specification = json!({"native_debugger_schema":crate::executors::session_host::SCHEMA,"debugger":start,"process":spec});
    session(request_id, ResourceKind::Debugger, specification)
}
pub(crate) fn interactive(params: &Map<String, Value>) -> Result<Plan> {
    let (request_id, start): (_, InteractiveStart) = start_parameters(params)?;
    ensure!(!start.argv.is_empty(), "interactive argv cannot be empty");
    ensure!(
        start.launch_spec.is_none() || start.env.is_empty(),
        "LaunchSpec owns interactive environment overrides"
    );
    let spec = Specification {
        argv: if start.launch_spec.is_some() {
            Vec::new()
        } else {
            start.argv
        },
        timeout_secs: start.launch_spec.is_none().then_some(3600),
        launch_spec: start.launch_spec,
        input: start.input,
        env: start.env,
        cwd: default_cwd(),
        output_limit: default_output(),
    };
    spec.validate()?;
    session(
        request_id,
        ResourceKind::Interactive,
        json!({"native_interactive_schema":crate::executors::session_host::SCHEMA,"process":spec}),
    )
}
fn session(request_id: &str, kind: ResourceKind, specification: Value) -> Result<Plan> {
    let intent = specification.clone();
    Ok(Plan {
        request_id: request_id.into(),
        specification,
        kind,
        response: crate::executors::plan::Response::Session,
        prepare: Box::new(move |inputs, owner| {
            let spec: Specification = serde_json::from_value(intent["process"].clone())?;
            let mode = if kind == ResourceKind::Interactive {
                crate::executors::session_request::Mode::Interactive
            } else if let Some(path) = intent["debugger"]["dump"].as_str() {
                crate::executors::session_request::Mode::Dump {
                    file: inputs.reference(owner, path, Purpose::Inspect)?,
                }
            } else {
                crate::executors::session_request::Mode::Launch
            };
            let request = crate::executors::session_request::Request {
                schema: crate::executors::session_host::SCHEMA,
                target: spec.prepare(inputs, owner)?,
                mode,
            };
            request.validate()?;
            Ok(crate::executors::plan::Target::Session(request))
        }),
    })
}
