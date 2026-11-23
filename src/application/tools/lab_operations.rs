//! Asynchronous host operations reuse durable tool-effect receipts.
use super::*;
use crate::application::windows_lab::WindowsLab;
use tokio_util::sync::CancellationToken;

pub(super) type Operations = Arc<Mutex<BTreeMap<String, CancellationToken>>>;
impl LabService {
    pub(super) async fn start_operation(
        &self,
        agent: &AgentMetadata,
        call: &str,
        name: &str,
        args: Value,
    ) -> Result<Value> {
        let vm = match name {
            "lab_prepare" => text(&args, "vm")?.to_owned(),
            "lab_restore" | "lab_install" => {
                self.resolve_reference(agent, text(&args, "lab_ref")?)?.vm
            }
            _ => bail!("unsupported lab operation"),
        };
        self.authorize_vm(agent, &vm)?;
        let lab = self.open(&vm, agent)?;
        let lab = if matches!(name, "lab_prepare" | "lab_restore") {
            lab.with_worker(Arc::new(self.assessment_worker(agent, call)))
        } else {
            lab
        };
        self.start_lab_operation(agent, call, name, args, lab).await
    }

    async fn start_lab_operation(
        &self,
        agent: &AgentMetadata,
        call: &str,
        name: &str,
        args: Value,
        lab: WindowsLab,
    ) -> Result<Value> {
        let operation_id = stable("lab-operation", &format!("{}:{call}", agent.id));
        let mut operations = self.operations.lock().await;
        if operations.contains_key(&operation_id) {
            return Ok(json!({"operation_id":operation_id,"state":"running"}));
        }
        let record = match DurableEffects::new(&self.store, self.output_limit).admit(
            agent,
            &operation_id,
            &format!("lab.operation.{name}"),
            &args,
            ReplayPolicy::OperatorOnly,
        )? {
            Admission::Execute(record) => record,
            Admission::Replay(outcome) => {
                return Ok(json!({"operation_id":operation_id,"result":outcome}));
            }
        };
        let token = CancellationToken::new();
        operations.insert(operation_id.clone(), token.clone());
        let service = self.clone();
        let actor = agent.clone();
        let name = name.to_owned();
        let id = operation_id.clone();
        tokio::spawn(async move {
            let action = service.perform(&lab, &actor, &name, args);
            let result =
                tokio::select! { result = action => Some(result), _ = token.cancelled() => None };
            let (raw, error, state) = match result {
                Some(Ok(value)) => (value, false, CallState::Completed),
                Some(Err(error)) => {
                    let unknown = error.is::<crate::application::windows_lab::LabError>();
                    (
                        json!({"code":if unknown {"lab_effect_unknown"} else {"lab_operation_rejected"}, "operation_id":id, "error":format!("{error:#}"), "details":error.downcast_ref::<crate::application::windows_lab::LabError>(), "effect_state":if unknown {"unknown"} else {"rejected"}, "next_action":if unknown {"inspect lab_status and reconcile the existing operation"} else {"correct the rejected input"}}),
                        true,
                        if unknown {
                            CallState::Uncertain
                        } else {
                            CallState::Failed
                        },
                    )
                }
                None => {
                    // The cancelled future may not have acquired the VM yet.
                    // Preserve its durable intent without rewriting another operation's state.
                    (
                        json!({"code":"lab_operation_cancelled","operation_id":id,"effect_state":"unknown","next_action":"inspect guest state and reconcile; cancellation does not prove rollback"}),
                        true,
                        CallState::Uncertain,
                    )
                }
            };
            if let Err(error) = DurableEffects::new(&service.store, service.output_limit)
                .finish(record, raw, error, state)
            {
                tracing::error!(%error, operation_id=%id, "cannot persist lab operation result");
            }
            service.operations.lock().await.remove(&id);
        });
        Ok(
            json!({"operation_id":operation_id,"state":"running","next_action":"query lab_operation_status; use lab_operation_cancel to request interruption"}),
        )
    }
    async fn perform(
        &self,
        lab: &WindowsLab,
        agent: &AgentMetadata,
        name: &str,
        args: Value,
    ) -> Result<Value> {
        if matches!(name, "lab_restore" | "lab_install") {
            let reference = self.resolve_reference(agent, text(&args, "lab_ref")?)?;
            lab.validate_lease(&reference.lab_id, reference.generation)?;
        }
        if matches!(name, "lab_restore" | "lab_install")
            && let Some(recorded) = lab.recorded_run()?
            && let Some(job_id) = recorded.job_id
            && let Some(job) = self.store.get_record::<JobRecord>(&job_id)?
        {
            ensure!(
                job.run_id == agent.run_id && job.agent_id == agent.id && job.evidence_ready(),
                "finish the owning lab job and synchronize its evidence before VM maintenance"
            );
            lab.record_job_completion(&job_id, &job.status).await?;
        }
        // Release only this assessment's idle lease before requesting exclusive
        // VM admission. Worker release refuses live jobs/sessions. A concurrent
        // new binding wins the shared file lock and makes maintenance fail closed.
        ensure!(
            self.store
                .records::<JobRecord>(&agent.run_id)?
                .iter()
                .all(JobRecord::evidence_ready),
            "finish job cleanup and evidence synchronization before VM maintenance"
        );
        if let Some(owner) = self.store.controller_owner()
            && let Some(environment) = &agent.environment_id
        {
            self.environments
                .release_scope(environment, &agent.run_id, &owner)
                .await?;
        }
        match name {
            "lab_prepare" => {
                let input: args::LabPrepareArgs = serde_json::from_value(args)?;
                let lease = lab
                    .prepare(
                        &self.owner(agent),
                        &input.purpose,
                        self.materialize_spec(agent, input.spec)?,
                    )
                    .await?;
                let value = serde_json::to_value(&lease)?;
                let reference = self
                    .bind_reference(
                        agent,
                        &input.vm,
                        text(&value, "lab_id")?,
                        value["generation"]
                            .as_u64()
                            .context("lease generation missing")?,
                    )
                    .map_err(|error| crate::application::windows_lab::LabError {
                        code: "lab_reference_persistence_failed",
                        operation_id: lease.lab_id.clone(),
                        generation: lease.generation,
                        receipt: None,
                        message: format!(
                            "preparation completed but reference publication failed: {error:#}"
                        ),
                    })?;
                Ok(json!({"lab_ref": reference, "lease": value}))
            }
            "lab_install" => {
                let input: args::LabInstallArgs = serde_json::from_value(args)?;
                let reference = self.resolve_reference(agent, &input.lab_ref)?;
                lab.install(
                    &reference.lab_id,
                    reference.generation,
                    input.timeout_secs.unwrap_or(300),
                )
                .await
            }
            "lab_restore" => {
                let input: args::LabRestoreArgs = serde_json::from_value(args)?;
                let reference = self.resolve_reference(agent, &input.lab_ref)?;
                lab.restore(&reference.lab_id, reference.generation).await
            }
            _ => bail!("unsupported lab operation"),
        }
    }
    pub(super) async fn operation(
        &self,
        agent: &AgentMetadata,
        id: &str,
        cancel: bool,
    ) -> Result<Value> {
        let record: ToolCallRecord = require_in_run(&self.store, id, &agent.run_id)?;
        ensure!(
            record.name.starts_with("lab.operation."),
            "not a lab operation handle"
        );
        let operations = self.operations.lock().await;
        if cancel {
            ensure!(
                record.agent_id == agent.id,
                "only the operation owner may cancel it"
            );
            if let Some(token) = operations.get(id) {
                token.cancel();
            }
        }
        let running = matches!(record.state, CallState::Prepared | CallState::Running);
        let interrupted = running && !operations.contains_key(id);
        Ok(
            json!({"operation_id":id,"state":if interrupted {CallState::Uncertain} else {record.state},"result":record.result,"evidence_id":record.evidence_id,"cancel_requested":cancel && operations.contains_key(id),"next_action":if interrupted {"controller ownership was lost; inspect lab_status and reconcile the existing operation"} else {"retain this handle and its evidence"}}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::windows_lab::VmHost;
    struct Host {
        entered: tokio::sync::Notify,
        calls: std::sync::atomic::AtomicUsize,
    }
    #[async_trait::async_trait]
    impl VmHost for Host {
        async fn running(&self) -> Result<bool> {
            Ok(false)
        }
        async fn snapshots(&self) -> Result<Vec<String>> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.entered.notify_one();
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok(vec![])
        }
        async fn snapshot(&self, _: &str) -> Result<()> {
            Ok(())
        }
        async fn revert(&self, _: &str) -> Result<()> {
            Ok(())
        }
        async fn start(&self) -> Result<()> {
            Ok(())
        }
        async fn stop(&self) -> Result<()> {
            Ok(())
        }
        async fn stop_clean(&self) -> Result<()> {
            Ok(())
        }
        async fn delete_snapshot(&self, _: &str) -> Result<()> {
            Ok(())
        }
    }
    #[tokio::test]
    async fn host_operations_return_handles_cancel_durably_and_do_not_replay() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let target = dir.path().join("target");
        fs::write(&target, b"trusted fixture")?;
        let config = AppConfig::default();
        let store = Store::open(dir.path().join("state"))?;
        let coordinator = Coordinator::new(store.clone());
        let run = coordinator.create_run(
            &target,
            "lab operation test",
            Budget::default(),
            serde_json::to_value(&config)?,
        )?;
        let agent: AgentMetadata = store.load(&run.root_id)?;
        let tools = ToolService::new(config, store.clone()).await?;
        let service = ToolContext::new(&tools, None).lab;
        let host = Arc::new(Host {
            entered: tokio::sync::Notify::new(),
            calls: 0.into(),
        });
        let vm = crate::config::WindowsLabVm {
            vmx: dir.path().join("fixture.vmx"),
            vmrun: dir.path().join("vmrun.exe"),
            ..Default::default()
        };
        let open = || {
            WindowsLab::open(store.root(), "fixture", &vm, host.clone())
                .map(|lab| lab.with_caller(service.owner(&agent)))
        };
        let args = json!({"vm":"fixture","purpose":"cancel regression","spec":{"launch":"exe","input_channel":"stdin","account":"user","dependencies":[]}});
        let handle = service
            .start_lab_operation(&agent, "host-call", "lab_prepare", args.clone(), open()?)
            .await?;
        let id = handle["operation_id"].as_str().unwrap();
        tokio::time::timeout(Duration::from_secs(5), host.entered.notified()).await?;
        assert_eq!(
            service.operation(&agent, id, false).await?["state"],
            "running"
        );
        assert!(coordinator.finish(&agent.id, "premature").is_err());
        assert_eq!(
            service.operation(&agent, id, true).await?["cancel_requested"],
            true
        );
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if store.load::<ToolCallRecord>(id).unwrap().state == CallState::Uncertain
                    && service.operations.lock().await.is_empty()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        let replay = service
            .start_lab_operation(&agent, "host-call", "lab_prepare", args, open()?)
            .await?;
        assert!(replay.get("result").is_some());
        assert_eq!(host.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(coordinator.finish(&agent.id, "uncertain").is_err());
        tools.close().await;
        Ok(())
    }
}
