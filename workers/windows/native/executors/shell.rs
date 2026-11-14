use crate::artifacts::inputs::Owner;
use crate::executors::resource_process::Interpreter;
use crate::executors::specification::{Specification, default_cwd};
use crate::executors::specification::{default_output, default_timeout};
use crate::{
    artifacts::resource_input::Purpose,
    executors::parameters::start_parameters,
    executors::plan::{Plan, Response},
};
use anyhow::{Result, ensure};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
// Synchronous shell replies over the existing immutable Python/process route.
// Only result observation waits; it never launches or repeats a command.

pub(crate) const SOURCE: &str = include_str!("shell_command.py");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Command {
    pub(crate) command: String,
    #[serde(default = "default_cwd")]
    pub(crate) cwd: String,
    pub(crate) target: Option<String>,
    #[serde(default)]
    pub(crate) env: BTreeMap<String, String>,
    #[serde(default = "default_timeout")]
    pub(crate) timeout_secs: u64,
    #[serde(default = "default_output")]
    pub(crate) output_limit: usize,
}

impl Command {
    pub(crate) fn configuration(&self) -> Result<Value> {
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
        crate::executors::resource_process::validate_environment(&self.env)?;
        Ok(json!({"schema":1,"command":self.command,"cwd":self.cwd,"env":self.env}))
    }
    pub(crate) fn process(&self, configuration: &Value) -> Result<Specification> {
        let source_hash = hex::encode(Sha256::digest(SOURCE.as_bytes()));
        let config_hash = crate::artifacts::inputs::digest(configuration)?;
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
    pub(crate) owner: Owner,
    pub(crate) resource: String,
    pub(crate) command: String,
}

pub(crate) fn plan(params: &Map<String, Value>) -> Result<Plan> {
    let (request_id, command): (_, Command) = start_parameters(params)?;
    let configuration = command.configuration()?;
    let spec = command.process(&configuration)?;
    let specification = json!({"native_process_schema":crate::executors::resource_process::SCHEMA,
            "native_shell_schema":1,"process":spec,"interpreter":Interpreter::Python});

    Ok(Plan {
        kind: crate::resources::models::ResourceKind::Process,
        request_id: request_id.into(),
        specification,
        response: Response::Command(command.command.clone()),
        prepare: Box::new(move |inputs, owner| {
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
                .map(crate::executors::plan::Target::Process)
        }),
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::state::tests::{fixture, owner};
    use crate::resources::{
        journal::{Cleanup, Outcome, Reservation},
        models::ResourceKind,
        state::Dispatch,
    };
    use crate::{artifacts::inputs::Owner, isolation::security::AclPolicy};
    use anyhow::Context;
    use std::fs;
    use std::io::Read;

    fn specification(command: &Command) -> Result<Value> {
        Ok(
            json!({"native_process_schema":crate::executors::resource_process::SCHEMA,
            "native_shell_schema":1,"process":command.process(&command.configuration()?)?,
            "interpreter":Interpreter::Python}),
        )
    }

    #[test]
    fn invalid_shell_parameters_fail_before_input_publication_or_reservation() -> Result<()> {
        let (root, inputs, mut runtime) = fixture()?;
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
                crate::resources::state::tests::planned(
                    &mut runtime,
                    &inputs,
                    &owner(),
                    crate::executors::shell::plan(params.as_object().unwrap())
                )
                .is_err()
            );
            assert!(runtime.test_journal().idle()?);
        }
        assert_eq!(fs::read_dir(root.path().join("native-inputs"))?.count(), 0);
        Ok(())
    }

    #[test]
    fn shell_text_is_immutable_data_not_interpreter_source_or_protected_argv() -> Result<()> {
        let (_root, inputs, _runtime) = fixture()?;
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
        let (root, inputs, mut runtime) = fixture()?;
        let params = json!({"command":"echo original","target":"files/target.exe","request_id":"shell-start"});
        let (_, command): (_, Command) = start_parameters(params.as_object().unwrap())?;
        let Reservation::Reserved {
            effect, resource, ..
        } = runtime.test_journal().reserve_start(
            &owner(),
            "shell-start",
            ResourceKind::Process,
            &specification(&command)?,
        )?
        else {
            anyhow::bail!("shell reservation absent")
        };
        runtime.test_journal().begin(&effect)?;
        let successor = Owner {
            epoch: 2,
            owner_id: "successor".into(),
            ..owner()
        };
        let Dispatch::Command(pending) = crate::resources::state::tests::planned(
            &mut runtime,
            &inputs,
            &successor,
            crate::executors::shell::plan(params.as_object().unwrap()),
        )?
        else {
            anyhow::bail!("interrupted shell must remain pending cleanup")
        };
        assert_eq!(pending.resource, resource);
        assert!(runtime.test_journal().begin(&effect).is_err());
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
                crate::resources::state::tests::planned(
                    &mut runtime,
                    &inputs,
                    &successor,
                    crate::executors::shell::plan(changed.as_object().unwrap())
                )
                .is_err(),
                "{field}"
            );
        }
        assert_eq!(fs::read_dir(root.path().join("native-inputs"))?.count(), 0);
        assert!(!runtime.test_journal().idle()?);
        Ok(())
    }

    #[test]
    fn closed_shell_retries_observe_original_status_without_reopening_inputs() -> Result<()> {
        let (root, inputs, mut runtime) = fixture()?;
        let params = json!({"command":"echo original","target":"files/missing.exe","request_id":"shell-closed"});
        let (_, command): (_, Command) = start_parameters(params.as_object().unwrap())?;
        let Reservation::Reserved {
            effect, resource, ..
        } = runtime.test_journal().reserve_start(
            &owner(),
            "shell-closed",
            ResourceKind::Process,
            &specification(&command)?,
        )?
        else {
            anyhow::bail!("shell reservation absent")
        };
        let ticket = runtime.test_journal().begin(&effect)?;
        runtime.test_journal().complete(
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
        crate::artifacts::io::publish_json(
            &root
                .path()
                .join("metadata/resource-results")
                .join(format!("{resource}.status.json")),
            &status,
            &policy,
        )?;
        runtime.test_journal().start_closing(&resource)?;
        runtime.test_journal().close(
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
        let Dispatch::Command(pending) = crate::resources::state::tests::planned(
            &mut runtime,
            &inputs,
            &successor,
            crate::executors::shell::plan(params.as_object().unwrap()),
        )?
        else {
            anyhow::bail!("shell observation absent")
        };
        let value = runtime
            .poll_command(&pending)?
            .context("completed shell missing")?;
        assert_eq!(value["resource_id"], resource);
        assert!(value.get("job_id").is_none());
        assert_eq!(value["native_owner"], serde_json::to_value(owner())?);
        assert_eq!(value["stdout"], status["stdout"]);
        assert_eq!(value["duration_secs"], 0.5);
        assert_eq!(runtime.poll_command(&pending)?, Some(value));
        assert_eq!(fs::read_dir(root.path().join("native-inputs"))?.count(), 0);
        assert!(runtime.test_journal().idle()?);
        Ok(())
    }
}
