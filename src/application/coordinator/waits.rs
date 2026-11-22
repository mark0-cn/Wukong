use super::*;

impl Coordinator {
    /// Resolve dependency admission once. Later follow-ups do not revoke work
    /// already admitted from the predecessor results recorded in this event.
    pub fn release_dependencies(&self, run_id: &str) -> Result<usize> {
        self.store.write(|tx| {
            let mut all=tx.records::<AgentMetadata>(run_id)?;
            let mut count=0;
            loop {
                let mut changed=false;
                for index in 0..all.len() {
                    let mut agent=all[index].clone();
                    if agent.status!=Status::Queued || agent.predecessors_released {continue;}
                    let state=DependencyRules::evaluate(agent.dependency_policy,&agent.wait_for_predecessors,&all);
                    match &state {
                        DependencyState::Pending=>continue,
                        DependencyState::Satisfied=>agent.predecessors_released=true,
                        DependencyState::Blocked{reason}=>{
                            agent.status=Status::Failed;
                            agent.error=Some(reason.clone());
                            let result=TaskResult{outcome:TaskOutcome::Blocked,answer:reason.clone(),next_actions:vec!["parent must revise the dependency, retry the prerequisite or choose a fallback".into()],..Default::default()};
                            agent.summary=Some(result.answer.clone());
                            agent.task_result=Some(result.clone());
                            tx.archive_result(&agent,&result)?;
                            Self::notify_terminal(tx,&agent)?;
                        }
                    }
                    agent.updated_at=now();
                    tx.save_record(&agent)?;
                    let predecessors:Vec<_>=all.iter().filter(|other|agent.wait_for_predecessors.contains(&other.id)).map(|other|json!({"id":other.id,"generation":other.generation,"status":other.status,"outcome":other.task_result.as_ref().map(|result|result.outcome)})).collect();
                    tx.event(run_id,Some(&agent.id),"agent.dependencies_resolved",json!({"policy":agent.dependency_policy,"result":state,"predecessors":predecessors}))?;
                    all[index]=agent;changed=true;count+=1;
                }
                if !changed {break;}
            }
            Ok(count)
        })
    }
    pub fn wait(&self, agent_id: &str, targets: &[String], timeout_seconds: u64) -> Result<Value> {
        self.wait_with_policy(
            agent_id,
            targets,
            timeout_seconds,
            DependencyPolicy::AllTerminal,
        )
    }
    pub fn wait_with_policy(
        &self,
        agent_id: &str,
        targets: &[String],
        timeout_seconds: u64,
        policy: DependencyPolicy,
    ) -> Result<Value> {
        self.store
            .write(|tx| Self::tx_wait(tx, agent_id, targets, timeout_seconds, policy))
    }
    pub(crate) fn tx_wait(
        tx: &crate::storage::StateSession<'_>,
        agent_id: &str,
        targets: &[String],
        timeout_seconds: u64,
        policy: DependencyPolicy,
    ) -> Result<Value> {
        let mut agent: AgentMetadata = tx
            .get_record::<AgentMetadata>(agent_id)?
            .context("agent missing")?;
        ensure!(!agent.status.terminal(), "agent is terminal");
        let all = tx.records::<AgentMetadata>(&agent.run_id)?;
        let ids: Vec<_> = if targets.is_empty() {
            all.iter()
                .filter(|other| other.parent_id.as_deref() == Some(agent_id))
                .map(|other| other.id.clone())
                .collect()
        } else {
            targets
                .iter()
                .map(|reference| TaskGraph::resolve(&all, reference).map(|agent| agent.id.clone()))
                .collect::<Result<_>>()?
        };
        ensure!(
            ids.len() <= 128 && ids.iter().collect::<HashSet<_>>().len() == ids.len(),
            "invalid or duplicate wait targets"
        );
        ensure!(
            !ids.iter().any(|id| id == agent_id),
            "cannot wait for yourself"
        );
        for id in &ids {
            ensure!(
                all.iter().any(|other| &other.id == id),
                "wait target not in this run"
            );
            let mut parent = agent.parent_id.as_deref();
            while let Some(pid) = parent {
                ensure!(pid != id, "cannot wait for an ancestor");
                parent = all
                    .iter()
                    .find(|other| other.id == pid)
                    .and_then(|other| other.parent_id.as_deref());
            }
        }
        let state = DependencyRules::evaluate(policy, &ids, &all);
        let states: Vec<_> = all
            .iter()
            .filter(|other| ids.contains(&other.id))
            .map(Self::wait_view)
            .collect();
        if state != DependencyState::Pending {
            return Ok(json!({"parked":false,"agents":states,"condition":state,"policy":policy}));
        }
        agent.status = Status::Waiting;
        agent.wait_for = ids;
        agent.wait_policy = policy;
        agent.wait_until = Some(now() + Duration::seconds(timeout_seconds.clamp(1, 3600) as i64));
        agent.updated_at = now();
        let mut graph = all;
        let index = graph
            .iter()
            .position(|other| other.id == agent.id)
            .context("agent missing")?;
        graph[index] = agent.clone();
        TaskGraph::validate(&graph)?;
        tx.save_record(&agent)?;
        tx.event(
            &agent.run_id,
            Some(agent_id),
            "agent.waiting",
            json!({"targets":agent.wait_for,"until":agent.wait_until,"policy":policy}),
        )?;
        Ok(
            json!({"parked":true,"agents":states,"wake_at":agent.wait_until,"condition":state,"policy":policy}),
        )
    }
    fn wait_view(agent: &AgentMetadata) -> Value {
        json!({"id":agent.id,"role":agent.role,"status":agent.status,"summary":agent.summary,"result":agent.task_result,"error":agent.error,"scope":agent.scope})
    }
    /// Give Root a bounded finalization window before the hard wall pause.
    /// The message is durable and deduplicated per controller epoch; waking a
    /// parked Root must not clear its real dependency condition or complete
    /// any descendant.
    pub fn request_finalization(
        &self,
        run_id: &str,
        controller: &str,
        remaining_wall_seconds: u64,
    ) -> Result<bool> {
        self.store.write(|tx| {
            let run: RunRecord = tx
                .get_record::<RunRecord>(run_id)?
                .context("run missing")?;
            let root: AgentMetadata = tx
                .get_record::<AgentMetadata>(&run.root_id)?
                .context("root missing")?;
            if run.status.terminal()
                || run.status == Status::BudgetPaused
                || root.status.terminal()
            {
                return Ok(false);
            }
            let content = json!({
                "type": "budget_finalization",
                "remaining_wall_seconds": remaining_wall_seconds,
                "instruction": "The wall-clock reserve has started. Do not create tasks, start probes, or park another wait. Stop descendants whose work is moot, preserve evidence and unresolved effects, mark unresolved coverage inconclusive, and call finish_scan now with verified conclusions, gaps, and next actions. Completion gates still apply; the hard wall pause remains in force."
            });
            let key = format!("budget-finalization:{run_id}:{controller}");
            let inserted = tx.message(
                run_id,
                "coordinator",
                &run.root_id,
                &content.to_string(),
                Some(&key),
            )?;
            if inserted {
                tx.event(
                    run_id,
                    Some(&run.root_id),
                    "run.finalization_requested",
                    json!({"controller":controller,"remaining_wall_seconds":remaining_wall_seconds}),
                )?;
            }
            Ok(inserted)
        })
    }

    pub fn wake_waiters(&self, run_id: &str) -> Result<usize> {
        self.store.write(|tx| {
            let all=tx.records::<AgentMetadata>(run_id)?;let mut changed=0;
            for mut agent in all.iter().filter(|agent|agent.status==Status::Waiting).cloned() {
                let condition=DependencyRules::evaluate(agent.wait_policy,&agent.wait_for,&all);
                let ready=condition!=DependencyState::Pending;
                let expired=agent.wait_until.is_some_and(|at|at<=now());
                let unread=tx.unread_messages(&agent.id,true)?;
                if !ready&&!expired&&unread==0 {continue;}
                if ready||expired {
                    let key=format!("wait:{}:{}:{}",agent.id,agent.generation,agent.wait_until.map(|date|date.timestamp_millis()).unwrap_or_default());
                    let states:Vec<_>=all.iter().filter(|other|agent.wait_for.contains(&other.id)).map(Self::wait_view).collect();
                    tx.message(run_id,"coordinator",&agent.id,&json!({"type":"wait_finished","timed_out":expired&&!ready,"condition":condition,"policy":agent.wait_policy,"agents":states,"instruction":"Inspect the dependency outcome and continue with tools: reconcile results, choose a fallback, or finish."}).to_string(),Some(&key))?;
                    agent.wait_for.clear();agent.wait_until=None;
                }
                agent.status=Status::Queued;agent.updated_at=now();
                tx.save_record(&agent)?;changed+=1;
            }
            Ok(changed)
        })
    }
}
