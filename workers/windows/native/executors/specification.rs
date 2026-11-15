//! Execution request preparation uses admitted inputs and owns no lifecycle or ledger.
use crate::{
    artifacts::inputs::{Inputs, Owner},
    artifacts::resource_input::Purpose,
    executors::resource_process::Request,
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Specification {
    #[serde(default)]
    pub(crate) argv: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) launch_spec: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) input: Option<String>,
    #[serde(default)]
    pub(crate) env: BTreeMap<String, String>,
    #[serde(default = "default_cwd")]
    pub(crate) cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) timeout_secs: Option<u64>,
    #[serde(default = "default_output")]
    pub(crate) output_limit: usize,
}

pub(crate) fn default_cwd() -> String {
    ".".into()
}
pub(crate) fn default_timeout() -> u64 {
    30
}
pub(crate) fn default_output() -> usize {
    1024 * 1024
}
impl Specification {
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            self.cwd == "."
                && (self.launch_spec.is_some() || !self.argv.is_empty())
                && self.argv.len() <= 129
                && self.argv.iter().all(|a| !a.contains('\0'))
                && self
                    .argv
                    .iter()
                    .map(|a| a.encode_utf16().count())
                    .sum::<usize>()
                    <= 16384,
            "native process requires argv and private cwd '.'"
        );
        Ok(())
    }
    pub(crate) fn matches(&self, request: &Request) -> bool {
        let launch = if let Some(launch) = &request.launch {
            self.launch_spec.as_ref() == Some(&launch.manifest.path)
                && self.argv.is_empty()
                && self.env.is_empty()
                && self
                    .timeout_secs
                    .is_none_or(|timeout| timeout == request.timeout_secs)
        } else {
            self.launch_spec.is_none()
                && self.argv.first() == Some(&request.program.path)
                && request.arguments == self.argv[1..]
                && request.environment == self.env
                && request.timeout_secs == self.timeout_secs.unwrap_or(default_timeout())
        };
        launch
            && request.input.as_ref().map(|input| input.path.as_str()) == self.input.as_deref()
            && request.output_limit == self.output_limit
    }
    pub(crate) fn prepare(&self, inputs: &Inputs, owner: &Owner) -> Result<Request> {
        self.prepare_interpreted(inputs, owner, None)
    }
    pub(crate) fn prepare_interpreted(
        &self,
        inputs: &Inputs,
        owner: &Owner,
        interpreter: Option<crate::executors::resource_process::Interpreter>,
    ) -> Result<Request> {
        self.validate()?;
        let input = self
            .input
            .as_deref()
            .map(|path| inputs.reference(owner, path, Purpose::Inspect))
            .transpose()?;
        let (program, arguments, environment, timeout, launch) =
            if let Some(path) = &self.launch_spec {
                ensure!(
                    self.argv.is_empty() && self.env.is_empty(),
                    "LaunchSpec owns argv and environment"
                );
                let (manifest, resolved) =
                    crate::executors::resource_launch::prepare(inputs, owner, path)?;
                ensure!(
                    self.timeout_secs
                        .is_none_or(|timeout| timeout == resolved.timeout_secs),
                    "LaunchSpec owns its execution timeout"
                );
                let binding = crate::executors::resource_launch::Binding {
                    manifest,
                    files: resolved.files,
                    cwd: resolved.cwd,
                    input_mode: resolved.input_mode,
                };
                (
                    resolved.program,
                    resolved.arguments,
                    resolved.environment,
                    resolved.timeout_secs,
                    Some(binding),
                )
            } else {
                (
                    inputs.reference(
                        owner,
                        &self.argv[0],
                        if interpreter.is_some() {
                            Purpose::Inspect
                        } else {
                            Purpose::Execute
                        },
                    )?,
                    self.argv[1..].to_vec(),
                    self.env.clone(),
                    self.timeout_secs.unwrap_or(default_timeout()),
                    None,
                )
            };
        let mut argument_files = BTreeMap::new();
        for path in arguments.iter().filter(|_| launch.is_none()) {
            if path.starts_with("files/")
                && path != &program.path
                && !argument_files.contains_key(path)
            {
                let reference = inputs.reference(owner, path, Purpose::Inspect)?;
                argument_files.insert(path.clone(), reference);
            }
        }
        let request = Request {
            schema: crate::executors::resource_process::SCHEMA,
            resource_id: "0".repeat(32),
            owner: owner.clone(),
            program,
            interpreter,
            replay: None,
            argument_files: argument_files.into_values().collect(),
            launch,
            arguments,
            input,
            environment,
            timeout_secs: timeout,
            output_limit: self.output_limit,
            processes: 16,
            memory_bytes: 1024 * 1024 * 1024,
        };
        request.validate()?;
        Ok(request)
    }
}
