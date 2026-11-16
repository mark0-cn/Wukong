use super::*;

impl Coordinator {
    pub fn create_run(
        &self,
        target: &Path,
        task: &str,
        budget: Budget,
        config: Value,
    ) -> Result<RunRecord> {
        let target_path =
            fs::canonicalize(target).context("target must be an existing file or directory")?;
        let (target_path, target_sha256, bundle) = if target_path.is_file() {
            let sha256 = crate::adapters::files::content::sha256_file(&target_path)?;
            (target_path, sha256, None)
        } else if target_path.is_dir() {
            let bundle = crate::application::bundle::load(&target_path)?;
            let entry = target_path.join(&bundle.entry);
            let sha256 = bundle.entry_sha256.clone();
            (entry, sha256, Some(bundle))
        } else {
            bail!("target is neither a file nor a directory");
        };
        let run_id = id("run");
        let root_id = id("agent");
        let timestamp = now();
        let run = RunRecord {
            id: run_id.clone(),
            target_path,
            target_sha256,
            bundle,
            task: task.to_owned(),
            status: Status::Running,
            root_id: root_id.clone(),
            config,
            budget,
            created_at: timestamp,
            updated_at: timestamp,
            total_tokens: 0,
            cost_usd: 0.0,
            summary: None,
        };
        let root = AgentSnapshot {
            metadata: AgentMetadata {
                id: root_id,
                run_id: run_id.clone(),
                parent_id: None,
                name: "Root".into(),
                role: Role::Root,
                task: task.to_owned(),
                scope: "assessment".into(),
                environment_id: run.config["default_environment"]
                    .as_str()
                    .map(str::to_owned),
                contract: TaskContract {
                    objective: task.into(),
                    ..Default::default()
                },
                task_result: None,
                skills: vec![],
                active_skills: vec![],
                status: Status::Queued,
                depth: 0,
                generation: 0,
                inherit_context: false,
                wait_for: vec![],
                wait_until: None,
                wait_policy: DependencyPolicy::AllTerminal,
                wait_for_predecessors: vec![],
                dependency_policy: DependencyPolicy::AllSuccess,
                predecessors_released: true,
                turns: 0,
                total_tokens: 0,
                cost_usd: 0.0,
                token_cap: run
                    .budget
                    .role_token_shares
                    .contains_key(&Role::Root)
                    .then(|| run.budget.role_token_cap(Role::Root)), // root is governed by the run budget alone unless an explicit root share is configured
                summary: None,
                error: None,
                created_at: timestamp,
                updated_at: timestamp,
            },
            inherited_context: None,
            checkpoint: None,
        };
        self.store.write(|tx| {
            tx.save_record(&run)?;
            tx.save_record(&root.metadata)?;
            tx.event(
                &run.id,
                Some(&root.id),
                "run.created",
                json!({"target_sha256":run.target_sha256,"bundle_sha256":run.bundle.as_ref().map(|bundle| &bundle.bundle_sha256),"task":task}),
            )?;
            Ok(())
        })?;
        Ok(run)
    }
    pub fn pause_budget(&self, run_id: &str, reason: &str) -> Result<()> {
        self.pause(run_id, reason, "budget")
    }
    pub fn pause_operator(&self, run_id: &str, reason: &str) -> Result<()> {
        self.pause(run_id, reason, "operator")
    }
    pub(crate) fn pause_interrupted(&self, run_id: &str, reason: &str) -> Result<()> {
        self.pause(run_id, reason, "interrupted")
    }
    fn pause(&self, run_id: &str, reason: &str, cause: &str) -> Result<()> {
        self.store.write(|tx| {
            let mut run: RunRecord = tx.get_record(run_id)?.context("run missing")?;
            if run.status.terminal() || run.status == Status::BudgetPaused {
                return Ok(());
            }
            run.status = Status::BudgetPaused;
            run.updated_at = now();
            tx.save_record(&run)?;
            for mut agent in tx.records::<AgentMetadata>(run_id)? {
                if !agent.status.terminal() {
                    agent.status = Status::BudgetPaused;
                    agent.updated_at = now();
                    tx.save_record(&agent)?;
                }
            }
            tx.event(
                run_id,
                None,
                if cause == "budget" {
                    "run.budget_paused"
                } else {
                    "run.paused"
                },
                json!({"reason":reason,"cause":cause}),
            )?;
            Ok(())
        })
    }
    pub fn resume(&self, run_id: &str, budget: Option<Budget>) -> Result<()> {
        self.resume_once(run_id, budget, &id("resume"))
    }
    /// One controller ownership epoch is one logical lifecycle transition.
    /// Replayed attempts cannot reset running agents or publish duplicate events.
    pub fn resume_once(&self, run_id: &str, budget: Option<Budget>, operation: &str) -> Result<()> {
        ensure!(
            !operation.is_empty() && operation.len() <= 256,
            "invalid resume operation identity"
        );
        let key = format!("{run_id}:resume:{operation}");
        let request = json!({"budget":budget});
        self.store.write(|tx| {
            if let Some(previous) = tx.get::<Value>("run_transition", &key)? {
                ensure!(
                    previous == request,
                    "resume operation identity reused with different arguments"
                );
                return Ok(());
            }
            let mut run: RunRecord = tx.get_record(run_id)?.context("run missing")?;
            if run.status.terminal() {
                bail!("terminal run cannot be resumed");
            }
            let first_start = run.status != Status::BudgetPaused
                && !tx.has_event(run_id, "controller.started")?
                && !tx.has_event(run_id, "run.started")?
                && !tx.has_event(run_id, "run.resumed")?;
            if let Some(budget) = budget {
                run.budget = budget;
            }
            run.status = Status::Running;
            run.updated_at = now();
            tx.save_record(&run)?;
            for mut a in tx.records::<AgentMetadata>(run_id)? {
                let mut changed = false;
                if !a.status.terminal() && a.role != Role::Root {
                    // Role caps follow the run budget: a resumed run with an
                    // enlarged budget rescales every surviving worker's quota.
                    // The root is governed by the run budget alone.
                    a.token_cap = Some(run.budget.role_token_cap(a.role));
                    changed = true;
                }
                if matches!(a.status, Status::Running | Status::BudgetPaused) {
                    a.status = if a.wait_until.is_some() {
                        Status::Waiting
                    } else {
                        Status::Queued
                    };
                    changed = true;
                }
                if changed {
                    a.updated_at = now();
                    tx.save_record(&a)?;
                }
            }
            tx.event(
                run_id,
                None,
                if first_start {
                    "run.started"
                } else {
                    "run.resumed"
                },
                json!({"operation_id":operation}),
            )?;
            tx.put("run_transition", &key, run_id, &request)?;
            Ok(())
        })
    }
}
