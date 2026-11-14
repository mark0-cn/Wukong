//! Python is a disposable analysis tool inside the same owned resource lifecycle.
use super::*;
use crate::resource_process::Interpreter;

const SOURCE: &str = include_str!("analysis_helper.py");
pub(crate) struct Pending {
    owner: Owner,
    resource: String,
}

impl Runtime {
    pub(super) fn start_analysis(
        &mut self,
        journal: &mut Journal,
        inputs: &Inputs,
        owner: &Owner,
        method: &str,
        params: &Map<String, Value>,
    ) -> Result<Dispatch> {
        let parameters = clean(params);
        let configuration = json!({"method":method,"params":parameters});
        let config_hash = crate::inputs::digest(&configuration)?;
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
        let intent = json!({"native_process_schema":crate::resource_process::SCHEMA,"native_analysis_schema":1,
            "process":spec,"interpreter":Interpreter::Python});
        let generated_id = format!("analysis-{config_hash}");
        let request = if params.contains_key("request_id") {
            request_id(params)?
        } else {
            &generated_id
        };
        let status = self.start_prepared(journal, owner, request, &intent, || {
            inputs.python_source(owner, SOURCE)?;
            inputs.generated(owner, config_path, serde_json::to_vec(&configuration)?)?;
            spec.prepare_interpreted(inputs, owner, Some(Interpreter::Python))
        })?;
        Ok(Dispatch::Analysis(Box::new(Pending {
            owner: owner.clone(),
            resource: status["job_id"]
                .as_str()
                .context("analysis resource missing")?
                .into(),
        })))
    }

    pub(crate) fn poll_analysis(
        &mut self,
        journal: &mut Journal,
        pending: &Pending,
    ) -> Result<Option<Value>> {
        self.poll(journal)?;
        let record = journal.resource(&pending.owner, &pending.resource)?;
        ensure!(
            record.specification["native_analysis_schema"] == 1,
            "resource is not an analysis invocation"
        );
        if record.phase != Phase::Closed {
            return Ok(None);
        }
        let status = self.status(journal, &pending.owner, &pending.resource)?;
        ensure!(
            status["state"] != "lost",
            "analysis outcome is unknown; reconcile the original resource"
        );
        let output = self.read_artifact(
            journal,
            &pending.owner,
            "file.get",
            json!({"path":format!("native-results/{}/stdout",pending.resource)})
                .as_object()
                .unwrap(),
        )?;
        let bytes = STANDARD.decode(
            output["data_base64"]
                .as_str()
                .context("analysis output missing")?,
        )?;
        let reply: Value =
            serde_json::from_slice(&bytes).context("analysis tool returned invalid JSON")?;
        ensure!(
            status["result"]["exit_code"] == 0 && reply.get("error").is_none(),
            "analysis failed: {}",
            reply["error"].as_str().unwrap_or("resource terminated")
        );
        let mut result = reply
            .get("result")
            .cloned()
            .filter(Value::is_object)
            .context("analysis result must be an object")?;
        result["execution_evidence"] = json!({"resource_id":pending.resource,"native_owner":record.owner,
            "stdout":output,"receipt":self.artifact(&pending.resource,"receipt")?});
        result["execution_evidence"]["stdout"]
            .as_object_mut()
            .unwrap()
            .remove("data_base64");
        Ok(Some(result))
    }
}
