use crate::executors::resource_process::Interpreter;
use crate::executors::specification::default_output;
use crate::executors::specification::{Specification, default_cwd};
use crate::{
    executors::parameters::start_parameters,
    executors::plan::{Plan, Response},
};
use anyhow::{Result, ensure};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
// Python source becomes a sealed input of the existing process resource.
// CPython executes only inside its target persona, never in the service.

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Python {
    pub(crate) source: String,
    #[serde(default)]
    pub(crate) argv: Vec<String>,
    pub(crate) target: Option<String>,
    pub(crate) timeout_secs: Option<u64>,
    pub(crate) output_limit: Option<usize>,
}
impl Python {
    pub(crate) fn process(&self) -> Result<Specification> {
        ensure!(
            !self.source.trim().is_empty()
                && self.source.len() <= 256 * 1024
                && !self.source.contains('\0')
                && self.argv.len() <= 64,
            "Python requires nonempty source <=256 KiB without NUL and at most 64 arguments"
        );
        let timeout = self.timeout_secs.unwrap_or(120);
        ensure!(
            (10..=3600).contains(&timeout),
            "Python timeout must be 10 through 3600 seconds"
        );
        let output_limit = self.output_limit.unwrap_or_else(default_output);
        ensure!(
            (1024..=8 * 1024 * 1024).contains(&output_limit),
            "Python output limit must be 1 KiB through 8 MiB"
        );
        let digest = hex::encode(Sha256::digest(self.source.as_bytes()));
        let mut argv = vec![Interpreter::Python.source_path(&digest)];
        argv.extend(self.argv.iter().cloned());
        if let Some(target) = &self.target {
            ensure!(
                target.starts_with("files/"),
                "Python target must be an admitted file"
            );
            argv.push(target.clone());
        }
        let spec = Specification {
            argv,
            launch_spec: None,
            input: None,
            env: BTreeMap::new(),
            cwd: default_cwd(),
            timeout_secs: Some(timeout),
            output_limit,
        };
        spec.validate()?;
        Ok(spec)
    }
}

pub(crate) fn plan(params: &Map<String, Value>) -> Result<Plan> {
    let (request_id, python): (_, Python) = start_parameters(params)?;
    let spec = python.process()?;
    let specification = json!({"native_process_schema":crate::executors::resource_process::SCHEMA,
            "process":spec,"interpreter":Interpreter::Python});

    Ok(Plan {
        kind: crate::resources::models::ResourceKind::Process,
        request_id: request_id.into(),
        specification,
        response: Response::Job,
        prepare: Box::new(move |inputs, owner| {
            let source = inputs.python_source(owner, &python.source)?;
            ensure!(
                source.path == spec.argv[0],
                "Python source identity differs"
            );
            spec.prepare_interpreted(inputs, owner, Some(Interpreter::Python))
                .map(crate::executors::plan::Target::Process)
        }),
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifacts::resource_input::Purpose;
    use crate::resources::state::tests::{fixture, owner};
    use crate::resources::{journal::Reservation, models::ResourceKind};
    use std::fs;
    use std::io::Read;

    #[test]
    fn interrupted_python_reuses_intent_without_admitting_successor_source() -> Result<()> {
        let (root, inputs, mut runtime) = fixture()?;
        let params = json!({"source":"print('owned')", "request_id":"python-start"});
        let python: Python = serde_json::from_value(json!({"source":params["source"]}))?;
        let spec = python.process()?;
        let specification = json!({"native_process_schema":crate::executors::resource_process::SCHEMA,
            "process":spec,"interpreter":Interpreter::Python});
        let Reservation::Reserved {
            effect, resource, ..
        } = runtime.test_journal().reserve_start(
            &owner(),
            "python-start",
            ResourceKind::Process,
            &specification,
        )?
        else {
            anyhow::bail!("missing Python reservation")
        };
        runtime.test_journal().begin(&effect)?;
        let mut successor = owner();
        successor.epoch += 1;
        successor.owner_id = "successor".into();
        let status =
            crate::resources::state::tests::ready(crate::resources::state::tests::planned(
                &mut runtime,
                &inputs,
                &successor,
                crate::executors::python::plan(params.as_object().unwrap()),
            ))?;
        assert_eq!(status["job_id"], resource);
        assert_eq!(status["state"], "lost");
        assert_eq!(status["cleanup_pending"], true);
        assert_eq!(fs::read_dir(root.path().join("native-inputs"))?.count(), 0);
        assert!(runtime.test_journal().begin(&effect).is_err());
        let mut changed = params.clone();
        changed["source"] = json!("print('changed')");
        assert!(
            crate::resources::state::tests::ready(crate::resources::state::tests::planned(
                &mut runtime,
                &inputs,
                &successor,
                crate::executors::python::plan(changed.as_object().unwrap())
            ))
            .is_err()
        );
        let mut native = serde_json::to_value(&spec)?;
        native["request_id"] = json!("python-start");
        assert!(
            crate::resources::state::tests::ready(crate::resources::state::tests::planned(
                &mut runtime,
                &inputs,
                &successor,
                crate::executors::plan::process(native.as_object().unwrap())
            ))
            .is_err()
        );
        assert_eq!(fs::read_dir(root.path().join("native-inputs"))?.count(), 0);
        assert!(!runtime.test_journal().idle()?);
        Ok(())
    }

    #[test]
    fn source_admission_retains_exact_readonly_bytes_and_owner() -> Result<()> {
        let (_root, inputs, _runtime) = fixture()?;
        let python: Python = serde_json::from_value(json!({
            "source":"print('你好')\n", "argv":["-c", "raise RuntimeError('argument')"]
        }))?;
        let source = inputs.python_source(&owner(), &python.source)?;
        assert_eq!(source, inputs.python_source(&owner(), &python.source)?);
        assert!(!source.executable);
        assert!(inputs.acquire(&owner(), &source, Purpose::Execute).is_err());
        let mut data = Vec::new();
        inputs
            .acquire(&owner(), &source, Purpose::Inspect)?
            .read_to_end(&mut data)?;
        assert_eq!(data, python.source.as_bytes());
        let request =
            python
                .process()?
                .prepare_interpreted(&inputs, &owner(), Some(Interpreter::Python))?;
        assert_eq!(request.program, source);
        assert_eq!(request.arguments, python.argv);
        assert_eq!(request.interpreter, Some(Interpreter::Python));
        let mut foreign = owner();
        foreign.run_id = "different-run".into();
        assert!(inputs.acquire(&foreign, &source, Purpose::Inspect).is_err());
        assert!(
            crate::executors::session_request::Request {
                schema: crate::executors::session_host::SCHEMA,
                target: request.clone(),
                mode: crate::executors::session_request::Mode::Launch,
            }
            .validate()
            .is_err()
        );
        let mut executable_script = request;
        executable_script.program.executable = true;
        assert!(executable_script.validate().is_err());
        Ok(())
    }

    #[test]
    fn invalid_python_parameters_cannot_publish_source_or_reserve_work() -> Result<()> {
        let (root, inputs, mut runtime) = fixture()?;
        for parameters in [
            json!({"source":" \n"}),
            json!({"source":"pass\0"}),
            json!({"source":"x".repeat(256*1024+1)}),
            json!({"source":"pass","argv":vec!["x";65]}),
            json!({"source":"pass","timeout_secs":9}),
            json!({"source":"pass","output_limit":1}),
            json!({"source":"pass","target":"C:/secret.exe"}),
            json!({"source":"pass","interpreter":"C:/python.exe"}),
            json!({"source":"pass","env":{"PYTHONPATH":"outside"}}),
            json!({"source":"pass","call_id":"different"}),
        ] {
            let mut params = parameters;
            params["request_id"] = json!("invalid-python");
            assert!(
                crate::resources::state::tests::ready(crate::resources::state::tests::planned(
                    &mut runtime,
                    &inputs,
                    &owner(),
                    crate::executors::python::plan(params.as_object().unwrap())
                ))
                .is_err()
            );
            assert!(runtime.test_journal().idle()?);
        }
        // A bad output limit fails before publishing even an otherwise valid source.
        assert_eq!(fs::read_dir(root.path().join("native-inputs"))?.count(), 0);
        Ok(())
    }
}
