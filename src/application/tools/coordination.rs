//! Coordination tool operations.
use super::*;

pub(super) struct CoordinationService {
    pub(super) store: Store,
    pub(super) skills_dir: Option<PathBuf>,
    pub(super) external_tools: Vec<String>,
    pub(super) output_limit: usize,
}
impl CoordinationService {
    pub(super) fn effect(
        &self,
        agent: &AgentMetadata,
        call: &str,
        action: impl FnOnce(&crate::storage::StateSession<'_>) -> Result<Value>,
    ) -> Result<Value> {
        DurableEffects::new(&self.store, self.output_limit).commit(agent, call, action)
    }

    pub(super) fn authorize_control(
        &self,
        actor: &AgentMetadata,
        target: &AgentMetadata,
    ) -> Result<()> {
        ensure!(actor.run_id == target.run_id, "cross-run control denied");
        if actor.role == Role::Root || actor.id == target.id {
            return Ok(());
        }
        let all = self.store.records::<AgentMetadata>(&actor.run_id)?;
        if TaskGraph::is_descendant(target, &actor.id, &all) {
            return Ok(());
        }
        bail!("only your own descendants may be controlled")
    }

    pub(crate) fn create_agent(
        &self,
        agent: &AgentMetadata,
        call_id: &str,
        args: &Value,
    ) -> Result<Value> {
        let request: crate::application::coordinator::ChildRequest =
            serde_json::from_value(args.clone())?;
        ensure!(
            request.role != Role::Root && request.skills.len() <= 5,
            "invalid child role or skill count"
        );
        self.store
            .verify_evidence_links(&agent.run_id, &request.contract.input_evidence_ids)?;
        for skill in &request.skills {
            agents::validate_capability(skill)?;
            agents::load_skill(skill, self.skills_dir.as_deref())?;
        }
        self.effect(agent, call_id, |tx| {
            Ok(serde_json::to_value(Coordinator::tx_create_child(
                tx,
                &agent.id,
                request,
                stable("agent", call_id),
            )?)?)
        })
    }

    pub(super) fn activate_skill(
        &self,
        agent: &AgentMetadata,
        call_id: &str,
        name: &str,
    ) -> Result<Value> {
        agents::validate_capabilities(
            agent
                .skills
                .iter()
                .map(String::as_str)
                .chain(agent.active_skills.iter().map(|skill| skill.name.as_str()))
                .chain(std::iter::once(name)),
        )?;
        let content = agents::load_skill(name, self.skills_dir.as_deref())?;
        let sha256 = binary::sha256_bytes(content.as_bytes());
        let evidence = self.store.artifact(
            &agent.run_id,
            &agent.id,
            call_id,
            "skill.instructions",
            content.as_bytes(),
            "text/markdown",
            json!({"name":name,"sha256":sha256}),
        )?;
        self.effect(agent,call_id,|tx| {
            let mut current: AgentMetadata = tx.get_record::<AgentMetadata>(&agent.id)?.context("agent missing")?;
            ensure!(current.generation == agent.generation, "agent activation changed before skill load");
            agents::validate_capabilities(current.skills.iter().map(String::as_str)
                .chain(current.active_skills.iter().map(|skill| skill.name.as_str()))
                .chain(std::iter::once(name)))?;
            let snapshot = ActiveSkill { name:name.into(),sha256:sha256.clone(),content,evidence_id:evidence.id.clone() };
            if let Some(existing) = current.active_skills.iter_mut().find(|skill|skill.name==name) {
                *existing=snapshot;
            } else { current.active_skills.push(snapshot); }
            current.updated_at=now();
            tx.save_record(&current)?;
            tx.event(&agent.run_id,Some(&agent.id),"skill.activated",json!({"name":name,"sha256":sha256,"evidence_id":evidence.id}))?;
            Ok(json!({"name":name,"sha256":sha256,"evidence_id":evidence.id,"active":true,"applies":"next fresh model request"}))
        })
    }

    pub(super) fn send_message(
        &self,
        agent: &AgentMetadata,
        call_id: &str,
        args: &Value,
        follow_up: bool,
    ) -> Result<Value> {
        let mut options: MessageOptions = serde_json::from_value(json!({
            "kind":args.get("kind").cloned().unwrap_or(json!("information")),
            "correlation_id":args.get("correlation_id"),"reply_to":args.get("reply_to"),
            "recipient_generation":args.get("recipient_generation"),
            "evidence_ids":args.get("evidence_ids").cloned().unwrap_or(json!([]))
        }))?;
        if follow_up {
            options.kind = MessageKind::FollowUp;
        }
        self.effect(agent, call_id, |tx| {
            if follow_up && let Some(contract) = args.get("contract").filter(|v| !v.is_null()) {
                return Ok(serde_json::to_value(Coordinator::tx_follow_up_contract(
                    tx,
                    agent,
                    text(args, "agent_id")?,
                    text(args, "message")?,
                    options,
                    call_id,
                    serde_json::from_value(contract.clone())?,
                )?)?);
            }
            Ok(serde_json::to_value(Coordinator::tx_send(
                tx,
                &agent.id,
                text(args, "agent_id")?,
                text(args, "message")?,
                options,
                Some(call_id),
            )?)?)
        })
    }
}
