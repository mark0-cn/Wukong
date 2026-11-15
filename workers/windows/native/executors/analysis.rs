use crate::artifacts::inputs::Owner;
use crate::executors::resource_process::Interpreter;
use crate::executors::specification::{Specification, default_cwd};
use crate::{
    executors::parameters::{clean, request_id},
    executors::plan::{Plan, Response},
};
use anyhow::{Context, Result, ensure};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
// Python is a disposable analysis tool inside the same owned resource lifecycle.

pub(crate) const SOURCE: &str = include_str!("analysis_helper.py");
pub(crate) struct Pending {
    pub(crate) owner: Owner,
    pub(crate) resource: String,
}

pub(crate) fn plan(params: &Map<String, Value>) -> Result<Plan> {
    let parameters = clean(params);
    let method = "seccomp.analyze";
    let configuration = json!({"method":method,"params":parameters});
    let config_hash = crate::artifacts::inputs::digest(&configuration)?;
    let config_path = format!("files/native-analysis/{config_hash}.json");
    let input = parameters
        .get("filter")
        .and_then(Value::as_str)
        .context("analysis requires an admitted filter file")?;
    ensure!(
        input.starts_with("files/"),
        "analysis inputs must be admitted files"
    );
    let argv = vec![
        Interpreter::Python.source_path(&hex::encode(Sha256::digest(SOURCE.as_bytes()))),
        config_path.clone(),
        input.into(),
    ];
    let spec = Specification {
        argv,
        launch_spec: None,
        input: None,
        env: BTreeMap::new(),
        cwd: default_cwd(),
        timeout_secs: Some(120),
        output_limit: 8 * 1024 * 1024,
    };
    spec.validate()?;
    let specification = json!({"native_process_schema":crate::executors::resource_process::SCHEMA,"native_analysis_schema":1,
            "process":spec,"interpreter":Interpreter::Python});
    let generated_id = format!("analysis-{config_hash}");
    let request_id = if params.contains_key("request_id") {
        request_id(params)?
    } else {
        &generated_id
    };

    Ok(Plan {
        kind: crate::resources::models::ResourceKind::Process,
        request_id: request_id.into(),
        specification,
        response: Response::Analysis,
        prepare: Box::new(move |inputs, owner| {
            inputs.python_source(owner, SOURCE)?;
            inputs.generated(owner, config_path, serde_json::to_vec(&configuration)?)?;
            spec.prepare_interpreted(inputs, owner, Some(Interpreter::Python))
                .map(crate::executors::plan::Target::Process)
        }),
    })
}
