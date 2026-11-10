use super::*;

impl Coordinator {
    /// Informational messages never reopen a completed task or interrupt a wait.
    pub fn message(&self, sender: &str, recipient: &str, content: &str) -> Result<()> {
        self.send(sender, recipient, content, MessageOptions::default())
            .map(|_| ())
    }
    pub fn follow_up(&self, sender: &str, recipient: &str, content: &str) -> Result<()> {
        self.send(
            sender,
            recipient,
            content,
            MessageOptions {
                kind: MessageKind::FollowUp,
                ..Default::default()
            },
        )
        .map(|_| ())
    }
    pub fn send(
        &self,
        sender: &str,
        recipient: &str,
        content: &str,
        options: MessageOptions,
    ) -> Result<MessageReceipt> {
        self.store
            .write(|tx| Self::tx_send(tx, sender, recipient, content, options, None))
    }
    pub(crate) fn tx_send(
        tx: &crate::store::StateSession<'_>,
        sender: &str,
        recipient: &str,
        content: &str,
        mut options: MessageOptions,
        dedupe_key: Option<&str>,
    ) -> Result<MessageReceipt> {
        let mut target: AgentMetadata = tx
            .get_record::<AgentMetadata>(recipient)?
            .context("recipient missing")?;
        let run: RunRecord = tx.get_record(&target.run_id)?.context("run missing")?;
        ensure!(!run.status.terminal(), "assessment is terminal");
        let quota_failed_target = target.status == Status::Failed
            && target
                .error
                .as_deref()
                .is_some_and(|error| error.contains("budget"));
        ensure!(
            !target.status.terminal()
                || target.status == Status::Completed
                || (quota_failed_target && options.kind == MessageKind::FollowUp),
            "recipient is stopped or failed; create a new focused task (quota-exhausted children resume with follow_up_agent)"
        );
        let sender_generation = if sender != "operator" {
            let from: AgentMetadata = tx
                .get_record::<AgentMetadata>(sender)?
                .context("sender missing")?;
            ensure!(from.run_id == target.run_id, "cross-run message denied");
            ensure!(
                !from.status.terminal(),
                "terminal sender cannot issue new messages"
            );
            Some(from.generation)
        } else {
            None
        };
        for evidence_id in &options.evidence_ids {
            let evidence: Evidence = tx
                .get_record(evidence_id)?
                .context("message evidence missing")?;
            ensure!(
                evidence.run_id == target.run_id,
                "cross-run evidence denied"
            );
        }
        let original = if options.kind == MessageKind::Reply {
            let mid = options.reply_to.context("reply requires reply_to")?;
            Some(
                tx.messages(&target.run_id)?
                    .into_iter()
                    .find(|message| message.id == mid)
                    .context("reply target not in this run")?,
            )
        } else {
            None
        };
        let envelope = MessageRules::prepare(
            sender,
            &target,
            sender_generation,
            content,
            &mut options,
            original.as_ref(),
        )?;
        let stale = envelope.delivery == MessageDelivery::Stale;
        let generation = envelope
            .options
            .recipient_generation
            .context("recipient generation missing")?;
        let correlation = envelope
            .options
            .correlation_id
            .clone()
            .context("correlation missing")?;
        let (mid, inserted) = tx.enqueue_message(
            &target.run_id,
            sender,
            recipient,
            content,
            dedupe_key,
            &envelope,
        )?;
        let receipt = MessageReceipt {
            message_id: mid,
            delivery: envelope.delivery,
            recipient_generation: generation,
            correlation_id: correlation,
        };
        if !inserted || stale {
            return Ok(receipt);
        }
        if let Some(mut original) = original {
            original.envelope.delivery = MessageDelivery::Replied;
            original.envelope.replied_by = Some(mid);
            tx.replace_envelope(&original)?;
        }
        let quota_failed = target.status == Status::Failed
            && target
                .error
                .as_deref()
                .is_some_and(|error| error.contains("budget"));
        if options.kind == MessageKind::FollowUp
            && (target.status == Status::Completed || quota_failed)
        {
            // Keep the old conversation as background, but start a new
            // Rig run so its lifecycle call cannot be replayed as new work.
            let mut current = Some(target.clone());
            while let Some(mut agent) = current {
                let reopen_this = agent.status == Status::Completed
                    || (agent.id == target.id
                        && quota_failed
                        && target.error.as_deref() == agent.error.as_deref());
                if !reopen_this {
                    ensure!(!agent.status.terminal(), "ancestor is stopped or failed");
                    break;
                }
                let history: AgentSnapshot =
                    tx.agent_snapshot(&agent.id)?.context("agent missing")?;
                let mut conversation = history.into_conversation();
                agent.generation = agent
                    .generation
                    .checked_add(1)
                    .context("agent generation exhausted")?;
                tx.stale_mail_before(&agent.id, agent.generation)?;
                conversation.inherited_context = conversation
                    .checkpoint
                    .take()
                    .or(conversation.inherited_context);
                agent.status = if run.status == Status::BudgetPaused {
                    Status::BudgetPaused
                } else {
                    Status::Queued
                };
                agent.wait_for.clear();
                agent.wait_until = None;
                agent.wait_for_predecessors.clear();
                agent.predecessors_released = true;
                agent.error = None;
                agent.task_result = None;
                // Resuming the SAME agent must give it headroom: a fresh
                // role-quota slice on top of what it already spent, so a
                // quota-exhausted child continues instead of dying again on
                // its first request.
                if agent.token_cap.is_some() {
                    agent.token_cap = Some(
                        agent
                            .total_tokens
                            .saturating_add(run.budget.role_token_cap(agent.role)),
                    );
                }
                agent.updated_at = now();
                tx.save_record(&AgentSnapshot::from_parts(agent.clone(), conversation))?;
                tx.event(
                    &agent.run_id,
                    Some(&agent.id),
                    "agent.reopened",
                    json!({"generation":agent.generation,"recipient":recipient}),
                )?;
                if agent.id == recipient {
                    target = agent.clone();
                } else {
                    tx.message(&agent.run_id,
                        "coordinator",
                        &agent.id,
                        &format!(
                            "A completed descendant {recipient} was reopened for a follow-up. Reconcile its new result before finishing."
                        ),
                        None,
                    )?;
                }
                current = agent
                    .parent_id
                    .as_ref()
                    .map(|id| {
                        tx.get_record::<AgentMetadata>(id)?
                            .context("parent missing")
                    })
                    .transpose()?;
            }
            TaskGraph::validate(&tx.records::<AgentMetadata>(&target.run_id)?)?;
        } else if target.status == Status::Waiting && options.kind != MessageKind::Information {
            target.status = Status::Queued;
            // A question/reply interrupts execution, not the declared wait condition.
            if options.kind == MessageKind::FollowUp {
                target.wait_for.clear();
                target.wait_until = None;
            }
            target.updated_at = now();
            tx.save_record(&target)?;
        }
        tx.event(
            &target.run_id,
            Some(recipient),
            "message.enqueued",
            json!({"message_id":mid,"sender":sender,"kind":options.kind,"generation":generation}),
        )?;
        Ok(receipt)
    }
}
