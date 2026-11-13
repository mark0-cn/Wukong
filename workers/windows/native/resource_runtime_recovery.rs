//! Resource lifecycle reconciliation, sharing the runtime's held host controls.
use super::*;
impl Runtime {
    pub(super) fn recover(&mut self, journal: &mut Journal) -> Result<()> {
        if self.recovery.is_some() {
            return Ok(());
        }
        let Some(record) = journal.active()? else {
            return Ok(());
        };
        ensure!(
            self.running
                .as_ref()
                .is_none_or(|running| running.failed.is_some()),
            "cannot recover an executing native host"
        );
        let attempt = hex::encode(crate::accounts::random_bytes::<16>()?);
        let request = crate::resource_recovery::Request {
            schema: crate::resource_process::SCHEMA,
            attempt: attempt.clone(),
            resource: record,
        };
        request.validate()?;
        drop(self.running.take());
        let path = self.request_path(&request.resource.id, &format!("recovery-{attempt}.json"));
        let policy = AclPolicy::private_vm();
        deployment::publish_json(&path, &request, &policy)?;
        let log = policy.create_file(
            &self.request_path(&request.resource.id, &format!("recovery-{attempt}.log")),
        )?;
        let input = fs::File::open("NUL")?;
        let environment = crate::environment::system(&self.root)?;
        let host = Spawn {
            executable: &self.executable,
            arguments: &[
                "resource-recover".into(),
                self.installation.clone().into(),
                self.root.join("metadata/deployment.json").into_os_string(),
                path.into_os_string(),
            ],
            directory: &self.root,
            environment: &environment,
            log: &log,
            input: &input,
        }
        .system_control()?;
        self.recovery = Some(Recovering {
            request,
            host,
            deadline: Instant::now() + Duration::from_secs(90),
        });
        Ok(())
    }
    pub(super) fn poll_recovery(&mut self, journal: &mut Journal) -> Result<()> {
        let recovery = self
            .recovery
            .as_ref()
            .context("native recovery host vanished")?;
        if !recovery.host.exited() && Instant::now() < recovery.deadline {
            return Ok(());
        }
        ensure!(
            recovery.host.stop(Duration::from_secs(10))? == 0,
            "native cleanup host failed; retain the resource blocker"
        );
        let record = &recovery.request.resource;
        let artifact = format!("recovery-{}", recovery.request.attempt);
        let path = self.result_path(&record.id, &format!("{artifact}.json"));
        AclPolicy::private_vm().verify(&path, false)?;
        let bytes = deployment::read_bytes(&path, 65536)?;
        let result: Value = serde_json::from_slice(&bytes)?;
        ensure!(
            result["schema"] == crate::resource_process::SCHEMA
                && result["scope"] == "owned_resource_recovery"
                && result["attempt"] == recovery.request.attempt
                && result["resource_id"] == record.id
                && result["owner"] == serde_json::to_value(&record.owner)?
                && result["record_sha256"] == crate::inputs::digest(record)?
                && result["jobs_empty"] == true
                && result["profiles_removed"] == true
                && result["scratch_removed"] == true,
            "native recovery proof binding or cleanup differs"
        );
        if self.result_path(&record.id, "json").try_exists()? {
            let path = self.request_path(&record.id, "json");
            AclPolicy::private_vm().verify(&path, false)?;
            let request: Request = serde_json::from_slice(&deployment::read_bytes(&path, 65536)?)?;
            request.validate()?;
            let spec: Specification =
                serde_json::from_value(record.specification["process"].clone())?;
            ensure!(
                request.owner == record.owner
                    && request.resource_id == record.id
                    && spec.argv.first() == Some(&request.program.path)
                    && request.arguments == spec.argv[1..]
                    && request.environment == spec.env
                    && request.timeout_secs == spec.timeout_secs
                    && request.output_limit == spec.output_limit,
                "recovered result request differs from its journal intent"
            );
            self.finish_result(journal, &request)?;
        } else {
            self.finish_unknown(journal, record, &artifact, &bytes)?;
        }
        self.recovery = None;
        Ok(())
    }
    fn finish_unknown(
        &self,
        journal: &mut Journal,
        record: &ResourceRecord,
        artifact: &str,
        receipt: &[u8],
    ) -> Result<()> {
        let path = self.result_path(&record.id, "status.json");
        if path.try_exists()? {
            AclPolicy::private_vm().verify(&path, false)?;
            let prior: Value = serde_json::from_slice(&deployment::read_bytes(&path, 65536)?)?;
            ensure!(
                prior["job_id"] == record.id
                    && prior["native_owner"] == serde_json::to_value(&record.owner)?
                    && prior["state"] == "lost"
                    && prior["result"]["outcome"] == "unknown",
                "previous interrupted status differs"
            );
        } else {
            let value = json!({"job_id":record.id,"kind":"process","state":"lost","native_owner":record.owner,"cleanup_pending":false,"result":{"outcome":"unknown","jobs_empty":true,"profiles_removed":true,"scratch_removed":true,"recovery_artifact":artifact,"recovery_receipt_sha256":hex::encode(Sha256::digest(receipt))},"production_execution":false});
            self.retain(&path, &serde_json::to_vec(&value)?)?;
        }
        journal.start_closing(&record.id)?;
        journal.close(
            &record.id,
            Cleanup {
                jobs_empty: true,
                profiles_removed: true,
                scratch_removed: true,
            },
        )
    }
}
