//! Python source becomes a sealed input of the existing process resource.
//! CPython executes only inside its target persona, never in the service.
use super::*;
use crate::resource_process::Interpreter;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Python {
    source: String,
    #[serde(default)]
    argv: Vec<String>,
    target: Option<String>,
    timeout_secs: Option<u64>,
    output_limit: Option<usize>,
}
impl Python {
    fn process(&self) -> Result<Specification> {
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
impl Runtime {
    pub(super) fn start_python(
        &mut self,
        journal: &mut Journal,
        inputs: &Inputs,
        owner: &Owner,
        params: &Map<String, Value>,
    ) -> Result<Value> {
        let (request_id, python): (_, Python) = start_parameters(params)?;
        let spec = python.process()?;
        let specification = json!({"native_process_schema":crate::resource_process::SCHEMA,
            "process":spec,"interpreter":Interpreter::Python});
        // Lookup precedes source admission: a resumed controller never creates
        // a second input or replays a completed/interrupted execution.
        self.start_prepared(journal, owner, request_id, &specification, || {
            let source = inputs.python_source(owner, &python.source)?;
            ensure!(
                source.path == spec.argv[0],
                "Python source identity differs"
            );
            spec.prepare_interpreted(inputs, owner, Some(Interpreter::Python))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource_runtime::tests::owner;
    use std::io::Read;

    fn fixture() -> Result<(tempfile::TempDir, Journal, Inputs, Runtime)> {
        let root = tempfile::tempdir()?;
        let policy = AclPolicy::current_user()?;
        policy.apply(root.path(), true)?;
        let journal = Journal::with_policy(root.path(), owner().identity, policy.clone())?;
        let inputs = Inputs::open(root.path(), policy)?;
        let runtime = Runtime {
            root: root.path().into(),
            executable: root.path().join("never-execute"),
            installation: owner().identity.installation_id,
            running: None,
            recovery: None,
            debugger: None,
        };
        Ok((root, journal, inputs, runtime))
    }

    #[test]
    fn interrupted_python_reuses_intent_without_admitting_successor_source() -> Result<()> {
        let (root, mut journal, inputs, mut runtime) = fixture()?;
        let params = json!({"source":"print('owned')", "request_id":"python-start"});
        let python: Python = serde_json::from_value(json!({"source":params["source"]}))?;
        let spec = python.process()?;
        let specification = json!({"native_process_schema":crate::resource_process::SCHEMA,
            "process":spec,"interpreter":Interpreter::Python});
        let Reservation::Reserved {
            effect, resource, ..
        } = journal.reserve_start(
            &owner(),
            "python-start",
            ResourceKind::Process,
            &specification,
        )?
        else {
            anyhow::bail!("missing Python reservation")
        };
        journal.begin(&effect)?;
        let mut successor = owner();
        successor.epoch += 1;
        successor.owner_id = "successor".into();
        let status = runtime.start_python(
            &mut journal,
            &inputs,
            &successor,
            params.as_object().unwrap(),
        )?;
        assert_eq!(status["job_id"], resource);
        assert_eq!(status["state"], "lost");
        assert_eq!(status["cleanup_pending"], true);
        assert_eq!(fs::read_dir(root.path().join("native-inputs"))?.count(), 0);
        assert!(journal.begin(&effect).is_err());
        let mut changed = params.clone();
        changed["source"] = json!("print('changed')");
        assert!(
            runtime
                .start_python(
                    &mut journal,
                    &inputs,
                    &successor,
                    changed.as_object().unwrap()
                )
                .is_err()
        );
        let mut native = serde_json::to_value(&spec)?;
        native["request_id"] = json!("python-start");
        assert!(
            runtime
                .start(
                    &mut journal,
                    &inputs,
                    &successor,
                    native.as_object().unwrap()
                )
                .is_err()
        );
        assert_eq!(fs::read_dir(root.path().join("native-inputs"))?.count(), 0);
        assert!(!journal.idle()?);
        Ok(())
    }

    #[test]
    fn source_admission_retains_exact_readonly_bytes_and_owner() -> Result<()> {
        let (_root, _journal, inputs, _runtime) = fixture()?;
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
            crate::session_request::Request {
                schema: crate::session_host::SCHEMA,
                target: request.clone(),
                mode: crate::session_request::Mode::Launch,
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
        let (root, mut journal, inputs, mut runtime) = fixture()?;
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
                runtime
                    .start_python(&mut journal, &inputs, &owner(), params.as_object().unwrap())
                    .is_err()
            );
            assert!(journal.idle()?);
        }
        // A bad output limit fails before publishing even an otherwise valid source.
        assert_eq!(fs::read_dir(root.path().join("native-inputs"))?.count(), 0);
        Ok(())
    }
}
