use super::*;

impl Coordinator {
    pub fn set_status(&self, agent_id: &str, status: Status, error: Option<String>) -> Result<()> {
        self.store.write(|tx| {
            let mut agent: AgentMetadata = tx
                .get_record::<AgentMetadata>(agent_id)?
                .context("agent missing")?;
            if agent.status.terminal() {
                bail!("cannot transition terminal agent {}", agent.id);
            }
            agent.status = status;
            agent.error = error;
            agent.updated_at = now();
            tx.save_record(&agent)?;
            if status.terminal() {
                Self::notify_terminal(tx, &agent)?;
            }
            tx.event(
                &agent.run_id,
                Some(agent_id),
                "agent.status",
                json!({"status":status,"error":agent.error}),
            )?;
            Ok(())
        })
    }
    pub(crate) fn notify_terminal(
        tx: &crate::store::StateSession<'_>,
        agent: &AgentMetadata,
    ) -> Result<()> {
        if let Some(parent) = &agent.parent_id {
            let key = if agent.generation == 0 {
                format!("terminal:{}", agent.id)
            } else {
                format!("terminal:{}:{}", agent.id, agent.generation)
            };
            tx.message(&agent.run_id,&agent.id,parent,&json!({"type":"agent_finished","agent_id":agent.id,"generation":agent.generation,"status":agent.status,"summary":agent.summary,"result":agent.task_result,"error":agent.error}).to_string(),Some(&key))?;
        }
        Ok(())
    }
    pub fn finish(&self, agent_id: &str, summary: &str) -> Result<()> {
        self.finish_result(
            agent_id,
            TaskResult {
                answer: summary.into(),
                ..Default::default()
            },
        )
    }
    pub fn finish_result(&self, agent_id: &str, result: TaskResult) -> Result<()> {
        let agent: AgentMetadata = self.store.load::<AgentMetadata>(agent_id)?;
        self.store
            .verify_evidence_links(&agent.run_id, &result.evidence_links())?;
        self.store
            .write(|tx| Self::tx_finish_result(tx, agent_id, result))
    }
    pub(crate) fn tx_finish_result(
        tx: &crate::store::StateSession<'_>,
        agent_id: &str,
        result: TaskResult,
    ) -> Result<()> {
        let summary = &result.answer;
        if summary.trim().is_empty() {
            bail!("completion summary must be nonempty");
        }
        let mut agent: AgentMetadata = tx
            .get_record::<AgentMetadata>(agent_id)?
            .context("agent missing")?;
        if agent.status == Status::Completed {
            return Ok(());
        }
        if agent.status.terminal() {
            bail!("agent already terminated with {}", agent.status);
        }
        CompletionRules::validate_result(&agent, &result)?;
        Self::validate_evidence(tx, &agent.run_id, &result.evidence_links())?;
        CompletionRules::check_ready(&agent, &tx.completion_facts(&agent.run_id, &agent.id)?)?;
        agent.status = if result.outcome == TaskOutcome::Cancelled {
            Status::Stopped
        } else {
            Status::Completed
        };
        agent.task_result = Some(result.clone());
        tx.archive_result(&agent, &result)?;
        agent.summary = Some(summary.to_owned());
        agent.updated_at = now();
        tx.save_record(&agent)?;
        Self::notify_terminal(tx, &agent)?;
        if agent.role == Role::Root {
            // Independent verification gate: a flag value in any child's
            // accepted result must be reproduced by a verifier before the
            // assessment can close.
            let others: Vec<AgentMetadata> = tx.records::<AgentMetadata>(&agent.run_id)?;
            let claim = others.iter().any(|other| {
                other.id != agent.id
                    && other
                        .task_result
                        .as_ref()
                        .is_some_and(|result| flag_claims(&result.answer).concrete)
            });
            if claim {
                let verifications: Vec<Verification> = tx.records::<Verification>(&agent.run_id)?;
                ensure!(
                    verifications.iter().any(|verification| verification.passed),
                    "a child reported a flag value that no verifier has independently reproduced; spawn a verifier for the claim and record verification_record passed=true before finish_scan"
                );
            }
            let mut run: RunRecord = tx.get_record(&agent.run_id)?.context("run missing")?;
            run.status = agent.status;
            run.summary = Some(summary.to_owned());
            run.updated_at = now();
            tx.save_record(&run)?;
        }
        tx.event(
            &agent.run_id,
            Some(agent_id),
            "agent.completed",
            json!({"summary":summary}),
        )?;
        Ok(())
    }

    pub(super) fn validate_evidence(
        tx: &crate::store::StateSession<'_>,
        run_id: &str,
        links: &[String],
    ) -> Result<()> {
        ensure!(links.len() <= 128, "too many task evidence references");
        for link in links {
            let evidence: Evidence = tx.get_record(link)?.context("task evidence missing")?;
            ensure!(
                evidence.run_id == run_id,
                "task evidence belongs to another run"
            );
        }
        Ok(())
    }

    pub fn stop(&self, agent_id: &str, reason: &str) -> Result<Vec<String>> {
        self.store.write(|tx| Self::tx_stop(tx, agent_id, reason))
    }
    /// Transactional stop usable inside a tool-effect receipt, so a durable
    /// retry after an uncertain outcome replays the stored result instead of
    /// re-executing the lifecycle transition.
    pub(crate) fn tx_stop(
        tx: &crate::store::StateSession<'_>,
        agent_id: &str,
        reason: &str,
    ) -> Result<Vec<String>> {
        let root: AgentMetadata = tx
            .get_record::<AgentMetadata>(agent_id)?
            .context("agent missing")?;
        let all = tx.records::<AgentMetadata>(&root.run_id)?;
        let mut stopped = Vec::new();
        for mut agent in all
            .iter()
            .filter(|a| a.id == agent_id || TaskGraph::is_descendant(a, agent_id, &all))
            .cloned()
        {
            if agent.status.terminal() {
                continue;
            }
            agent.status = Status::Stopped;
            agent.error = Some(reason.to_owned());
            agent.updated_at = now();
            tx.save_record(&agent)?;
            Self::notify_terminal(tx, &agent)?;
            stopped.push(agent.id);
        }
        if root.role == Role::Root {
            let mut run: RunRecord = tx.get_record(&root.run_id)?.context("run missing")?;
            if !run.status.terminal() {
                run.status = Status::Stopped;
                run.updated_at = now();
                tx.save_record(&run)?;
            }
        }
        tx.event(
            &root.run_id,
            Some(agent_id),
            "agents.stopped",
            json!({"agents":stopped,"reason":reason}),
        )?;
        Ok(stopped)
    }
}
