use super::*;

impl Coordinator {
    pub fn create_child(&self, parent_id: &str, request: ChildRequest) -> Result<SpawnResult> {
        let parent: AgentMetadata = self.store.load::<AgentMetadata>(parent_id)?;
        self.store
            .verify_evidence_links(&parent.run_id, &request.contract.input_evidence_ids)?;
        self.store
            .write(|tx| Self::tx_create_child(tx, parent_id, request, id("agent")))
    }
    pub(crate) fn tx_create_child(
        tx: &crate::store::StateSession<'_>,
        parent_id: &str,
        mut request: ChildRequest,
        child_id: String,
    ) -> Result<SpawnResult> {
        if request.role == Role::Root {
            bail!("a child cannot have root role");
        }
        if request.name.trim().is_empty()
            || request.task.trim().is_empty()
            || request.scope.trim().is_empty()
        {
            bail!("name, task and scope must be nonempty");
        }
        if request.skills.len() > 5 {
            bail!("at most 5 related skills per agent");
        }
        crate::agents::validate_capabilities(request.skills.iter().map(String::as_str))?;
        let parent: AgentMetadata = tx.get_record(parent_id)?.context("parent agent missing")?;
        if parent.status.terminal() {
            bail!("parent is terminal");
        }
        let run: RunRecord = tx.get_record(&parent.run_id)?.context("run missing")?;
        if run.status.terminal() {
            bail!("run is terminal");
        }
        if request.contract.objective.trim().is_empty() {
            request.contract.objective = request.task.clone();
        }
        CompletionRules::validate_contract(&request.contract)?;
        Self::validate_evidence(tx, &run.id, &request.contract.input_evidence_ids)?;
        let agents = tx.records::<AgentMetadata>(&run.id)?;
        if let Some(failed) = agents.iter().find(|a| {
            a.scope.trim() == request.scope.trim()
                && a.role == request.role
                && a.status.terminal()
                && a.error
                    .as_deref()
                    .is_some_and(|error| error.contains("budget"))
        }) {
            bail!(
                "scope '{}' belongs to a quota-exhausted child ({}): resume it with follow_up_agent instead of spawning a replacement — its conversation and dossier are preserved",
                request.scope.trim(),
                failed.id
            );
        }
        if let Some(existing) = agents.iter().find(|a| {
            a.scope.trim() == request.scope.trim() && a.role == request.role && !a.status.terminal()
        }) {
            let mut existing_contract = existing.contract.clone();
            if existing_contract.objective.trim().is_empty() {
                existing_contract.objective = existing.task.clone();
            }
            ensure!(
                existing_contract == request.contract
                    && existing.dependency_policy == request.dependency_policy
                    && existing.wait_for_predecessors == request.wait_for_predecessors,
                "active scope has a different task contract; send a clarification or use a distinct scope"
            );
            return Ok(SpawnResult {
                created: false,
                agent: tx.get_record(&existing.id)?.context("agent missing")?,
            });
        }
        if agents.len() >= run.budget.max_agents {
            bail!("maximum total agent count reached");
        }
        if parent.depth + 1 > run.budget.max_depth {
            bail!("maximum agent depth reached");
        }
        ensure!(
            request.wait_for_predecessors.len() <= 128
                && request
                    .wait_for_predecessors
                    .iter()
                    .collect::<HashSet<_>>()
                    .len()
                    == request.wait_for_predecessors.len(),
            "invalid or duplicate predecessors"
        );
        for predecessor in &request.wait_for_predecessors {
            let target = agents
                .iter()
                .find(|a| &a.id == predecessor)
                .with_context(|| format!("wait_for_predecessors target missing: {predecessor}"))?;
            ensure!(
                target.run_id == run.id,
                "wait_for_predecessors target belongs to another run"
            );
        }
        let timestamp = now();
        let agent = AgentSnapshot {
            metadata: AgentMetadata {
                id: child_id,
                run_id: run.id.clone(),
                parent_id: Some(parent.id.clone()),
                name: request.name,
                role: request.role,
                task: request.task,
                scope: request.scope,
                contract: request.contract,
                task_result: None,
                skills: request.skills,
                active_skills: vec![],
                status: Status::Queued,
                depth: parent.depth + 1,
                generation: 0,
                inherit_context: request.inherit_context,
                wait_for: vec![],
                wait_until: None,
                wait_policy: DependencyPolicy::AllTerminal,
                wait_for_predecessors: request.wait_for_predecessors.clone(),
                dependency_policy: request.dependency_policy,
                predecessors_released: request.wait_for_predecessors.is_empty(),
                turns: 0,
                total_tokens: 0,
                cost_usd: 0.0,
                token_cap: Some(run.budget.role_token_cap(request.role)),
                summary: None,
                error: None,
                created_at: timestamp,
                updated_at: timestamp,
            },
            inherited_context: if request.inherit_context {
                tx.agent_conversation(&parent.id)?.checkpoint
            } else {
                None
            },
            checkpoint: None,
        };
        let mut graph = agents;
        graph.push(agent.metadata.clone());
        TaskGraph::validate(&graph)?;
        tx.save_record(&agent)?;
        tx.event(
            &run.id,
            Some(&agent.id),
            "agent.created",
            json!({"parent_id":parent.id,"role":agent.role,"scope":agent.scope}),
        )?;
        Ok(SpawnResult {
            created: true,
            agent,
        })
    }
}
