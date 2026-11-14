//! Synchronous shell replies over the existing immutable Python/process route.
//! Only result observation waits; it never launches or repeats a command.
use super::*;
use crate::resource_process::Interpreter;

const SOURCE: &str = include_str!("shell_command.py");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Command {
    command: String,
    #[serde(default = "default_cwd")]
    cwd: String,
    target: Option<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default = "default_timeout")]
    timeout_secs: u64,
    #[serde(default = "default_output")]
    output_limit: usize,
}

impl Command {
    fn configuration(&self) -> Result<Value> {
        ensure!(
            !self.command.trim().is_empty()
                && self.command.chars().count() <= 65536
                && !self.command.contains('\0'),
            "command requires 1 through 65536 characters without NUL"
        );
        ensure!(
            (1..=120).contains(&self.timeout_secs),
            "shell timeout must be 1 through 120 seconds"
        );
        ensure!(
            (1024..=8 * 1024 * 1024).contains(&self.output_limit),
            "shell output limit must be 1 KiB through 8 MiB"
        );
        if self.cwd != "." {
            wukong_worker_package::validate_path(&self.cwd)?;
        }
        crate::resource_process::validate_environment(&self.env)?;
        Ok(json!({"schema":1,"command":self.command,"cwd":self.cwd,"env":self.env}))
    }
    fn process(&self, configuration: &Value) -> Result<Specification> {
        let source_hash = hex::encode(Sha256::digest(SOURCE.as_bytes()));
        let config_hash = crate::inputs::digest(configuration)?;
        let mut argv = vec![
            Interpreter::Python.source_path(&source_hash),
            format!("files/native-shell/{config_hash}.json"),
        ];
        if let Some(target) = &self.target {
            ensure!(
                target.starts_with("files/"),
                "shell target must be an admitted executable"
            );
            argv.push(target.clone());
        }
        let spec = Specification {
            argv,
            launch_spec: None,
            input: None,
            env: BTreeMap::new(),
            cwd: default_cwd(),
            timeout_secs: Some(self.timeout_secs),
            output_limit: self.output_limit,
        };
        spec.validate()?;
        Ok(spec)
    }
}

pub(crate) struct Pending {
    owner: Owner,
    resource: String,
    command: String,
}
impl Runtime {
    pub(super) fn start_shell(
        &mut self,
        journal: &mut Journal,
        inputs: &Inputs,
        owner: &Owner,
        params: &Map<String, Value>,
    ) -> Result<Dispatch> {
        let (request_id, command): (_, Command) = start_parameters(params)?;
        let configuration = command.configuration()?;
        let spec = command.process(&configuration)?;
        let specification = json!({"native_process_schema":crate::resource_process::SCHEMA,
            "native_shell_schema":1,"process":spec,"interpreter":Interpreter::Python});
        let status = self.start_prepared(journal, owner, request_id, &specification, || {
            if let Some(target) = &command.target {
                inputs.reference(owner, target, Purpose::Execute)?;
            }
            let source = inputs.python_source(owner, SOURCE)?;
            let config = inputs.shell_command(owner, &configuration)?;
            ensure!(
                source.path == spec.argv[0] && config.path == spec.argv[1],
                "shell source/configuration identity differs"
            );
            spec.prepare_interpreted(inputs, owner, Some(Interpreter::Python))
        })?;
        let pending = Pending {
            owner: owner.clone(),
            resource: status["job_id"]
                .as_str()
                .context("native shell resource ID absent")?
                .into(),
            command: command.command,
        };
        Ok(Dispatch::Command(Box::new(pending)))
    }
    pub(crate) fn poll_command(
        &mut self,
        journal: &mut Journal,
        pending: &Pending,
    ) -> Result<Option<Value>> {
        self.poll(journal)?;
        let record = journal.resource(&pending.owner, &pending.resource)?;
        ensure!(
            record.kind == ResourceKind::Process
                && record.specification["native_shell_schema"] == 1,
            "native command observation does not belong to its shell resource"
        );
        if record.phase != Phase::Closed {
            return Ok(None);
        }
        let status = self.status(journal, &pending.owner, &pending.resource)?;
        let result = &status["result"];
        let unknown = status["state"] == "lost";
        let artifact = if unknown {
            result["recovery_artifact"]
                .as_str()
                .context("native shell recovery receipt absent")?
        } else {
            "receipt"
        };
        Ok(Some(
            json!({"state":status["state"],"command":pending.command,"engine":"cmd",
            "resource_id":pending.resource,"native_owner":status["native_owner"],
            "exit_code":result["exit_code"],"signal":null,"duration_secs":result["duration_secs"],
            "stdout":status["stdout"],"stderr":status["stderr"],
            "output_bytes_retained":result["output_bytes_retained"],"output_truncated":result["output_truncated"],
            "receipt":format!("native-results/{}/{artifact}",pending.resource),
            "stdout_artifact":if unknown {None}else{Some(format!("native-results/{}/stdout",pending.resource))},
            "stderr_artifact":if unknown {None}else{Some(format!("native-results/{}/stderr",pending.resource))},
            "cleanup_pending":false,"result":result,"production_execution":false}),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource_runtime::tests::{fixture, owner};
    use std::io::Read;

    fn specification(command: &Command) -> Result<Value> {
        Ok(
            json!({"native_process_schema":crate::resource_process::SCHEMA,
            "native_shell_schema":1,"process":command.process(&command.configuration()?)?,
            "interpreter":Interpreter::Python}),
        )
    }

    #[test]
    fn invalid_shell_parameters_fail_before_input_publication_or_reservation() -> Result<()> {
        let (root, mut journal, inputs, mut runtime) = fixture()?;
        for values in [
            json!({"command":" \n"}),
            json!({"command":"echo\0"}),
            json!({"command":"x".repeat(65537)}),
            json!({"command":"echo ok","timeout_secs":0}),
            json!({"command":"echo ok","timeout_secs":121}),
            json!({"command":"echo ok","output_limit":1023}),
            json!({"command":"echo ok","cwd":"../outside"}),
            json!({"command":"echo ok","cwd":"C:/outside"}),
            json!({"command":"echo ok","target":"C:/outside.exe"}),
            json!({"command":"echo ok","env":{"PATH":"outside"}}),
            json!({"command":"echo ok","env":{"COMSPEC":"outside"}}),
            json!({"command":"echo ok","env":{"AFL_TEST":"x\0"}}),
            json!({"command":"echo ok","interpreter":"powershell"}),
            json!({"command":"echo ok","call_id":"different"}),
        ] {
            let mut params = values;
            params["request_id"] = json!("invalid-shell");
            assert!(
                runtime
                    .start_shell(&mut journal, &inputs, &owner(), params.as_object().unwrap())
                    .is_err()
            );
            assert!(journal.idle()?);
        }
        assert_eq!(fs::read_dir(root.path().join("native-inputs"))?.count(), 0);
        Ok(())
    }

    #[test]
    fn shell_text_is_immutable_data_not_interpreter_source_or_protected_argv() -> Result<()> {
        let (_root, _journal, inputs, _runtime) = fixture()?;
        let command: Command = serde_json::from_value(json!({
            "command":"界".repeat(65536),"cwd":"work/nested",
            "env":(0..12).map(|i|(format!("AFL_TEST_{i}"),"z".repeat(4096))).collect::<BTreeMap<_,_>>()
        }))?;
        let configuration = command.configuration()?;
        let source = inputs.python_source(&owner(), SOURCE)?;
        let data = inputs.shell_command(&owner(), &configuration)?;
        assert_eq!(data, inputs.shell_command(&owner(), &configuration)?);
        assert!(!data.executable);
        assert!(inputs.acquire(&owner(), &data, Purpose::Execute).is_err());
        let mut stored = Vec::new();
        inputs
            .acquire(&owner(), &data, Purpose::Inspect)?
            .read_to_end(&mut stored)?;
        assert_eq!(stored, serde_json::to_vec(&configuration)?);
        assert!(stored.len() > 65536);
        let request = command.process(&configuration)?.prepare_interpreted(
            &inputs,
            &owner(),
            Some(Interpreter::Python),
        )?;
        assert_eq!(request.program, source);
        assert_eq!(request.argument_files, [data]);
        assert_eq!(request.arguments.len(), 1);
        assert!(request.environment.is_empty());
        assert!(serde_json::to_vec(&request)?.len() < 4096);
        let mut foreign = owner();
        foreign.run_id = "foreign".into();
        assert!(
            inputs
                .acquire(&foreign, &request.argument_files[0], Purpose::Inspect)
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn interrupted_shell_retries_keep_original_intent_and_reject_changed_configuration()
    -> Result<()> {
        let (root, mut journal, inputs, mut runtime) = fixture()?;
        let params = json!({"command":"echo original","target":"files/target.exe","request_id":"shell-start"});
        let (_, command): (_, Command) = start_parameters(params.as_object().unwrap())?;
        let Reservation::Reserved {
            effect, resource, ..
        } = journal.reserve_start(
            &owner(),
            "shell-start",
            ResourceKind::Process,
            &specification(&command)?,
        )?
        else {
            anyhow::bail!("shell reservation absent")
        };
        journal.begin(&effect)?;
        let successor = Owner {
            epoch: 2,
            owner_id: "successor".into(),
            ..owner()
        };
        let Dispatch::Command(pending) = runtime.start_shell(
            &mut journal,
            &inputs,
            &successor,
            params.as_object().unwrap(),
        )?
        else {
            anyhow::bail!("interrupted shell must remain pending cleanup")
        };
        assert_eq!(pending.resource, resource);
        assert!(journal.begin(&effect).is_err());
        for (field, value) in [
            ("command", json!("echo different")),
            ("cwd", json!("nested")),
            ("env", json!({"AFL_TEST":"different"})),
            ("target", json!("files/other.exe")),
            ("timeout_secs", json!(31)),
            ("output_limit", json!(2048)),
        ] {
            let mut changed = params.clone();
            changed[field] = value;
            assert!(
                runtime
                    .start_shell(
                        &mut journal,
                        &inputs,
                        &successor,
                        changed.as_object().unwrap()
                    )
                    .is_err(),
                "{field}"
            );
        }
        assert_eq!(fs::read_dir(root.path().join("native-inputs"))?.count(), 0);
        assert!(!journal.idle()?);
        Ok(())
    }

    #[test]
    fn closed_shell_retries_observe_original_status_without_reopening_inputs() -> Result<()> {
        let (root, mut journal, inputs, mut runtime) = fixture()?;
        let params = json!({"command":"echo original","target":"files/missing.exe","request_id":"shell-closed"});
        let (_, command): (_, Command) = start_parameters(params.as_object().unwrap())?;
        let Reservation::Reserved {
            effect, resource, ..
        } = journal.reserve_start(
            &owner(),
            "shell-closed",
            ResourceKind::Process,
            &specification(&command)?,
        )?
        else {
            anyhow::bail!("shell reservation absent")
        };
        let ticket = journal.begin(&effect)?;
        journal.complete(
            &ticket,
            &Outcome::Result {
                value: json!({"job_id":resource,"state":"running"}),
            },
        )?;
        let policy = AclPolicy::private_vm();
        policy.create_directory(&root.path().join("metadata"))?;
        policy.create_directory(&root.path().join("metadata/resource-results"))?;
        let status = json!({"job_id":resource,"state":"completed","native_owner":owner(),
            "stdout":"original\r\n","stderr":"","result":{"exit_code":0,"duration_secs":0.5,
            "output_bytes_retained":{"stdout":10,"stderr":0},"output_truncated":false}});
        deployment::publish_json(
            &runtime.result_path(&resource, "status.json"),
            &status,
            &policy,
        )?;
        journal.start_closing(&resource)?;
        journal.close(
            &resource,
            Cleanup {
                jobs_empty: true,
                profiles_removed: true,
                scratch_removed: true,
            },
        )?;
        let successor = Owner {
            epoch: 2,
            owner_id: "successor".into(),
            ..owner()
        };
        let Dispatch::Command(pending) = runtime.start_shell(
            &mut journal,
            &inputs,
            &successor,
            params.as_object().unwrap(),
        )?
        else {
            anyhow::bail!("shell observation absent")
        };
        let value = runtime
            .poll_command(&mut journal, &pending)?
            .context("completed shell missing")?;
        assert_eq!(value["resource_id"], resource);
        assert!(value.get("job_id").is_none());
        assert_eq!(value["native_owner"], serde_json::to_value(owner())?);
        assert_eq!(value["stdout"], status["stdout"]);
        assert_eq!(value["duration_secs"], 0.5);
        assert_eq!(runtime.poll_command(&mut journal, &pending)?, Some(value));
        assert_eq!(fs::read_dir(root.path().join("native-inputs"))?.count(), 0);
        assert!(journal.idle()?);
        Ok(())
    }
}
