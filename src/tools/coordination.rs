//! Coordination tool operations.
use super::*;

impl ToolService {
    pub(super) fn authorize_control(
        &self,
        actor: &AgentRecord,
        target: &AgentRecord,
    ) -> Result<()> {
        ensure!(actor.run_id == target.run_id, "cross-run control denied");
        if actor.role == Role::Root || actor.id == target.id {
            return Ok(());
        }
        let all = self
            .store
            .list_metadata::<AgentRecord>("agent", &actor.run_id)?;
        let mut parent = target.parent_id.as_deref();
        let mut count = 0;
        while let Some(id) = parent {
            if id == actor.id {
                return Ok(());
            }
            parent = all
                .iter()
                .find(|a| a.id == id)
                .and_then(|a| a.parent_id.as_deref());
            count += 1;
            if count > all.len() {
                break;
            }
        }
        bail!("only your own descendants may be controlled")
    }

    pub(crate) fn create_agent(
        &self,
        agent: &AgentRecord,
        call_id: &str,
        args: &Value,
    ) -> Result<Value> {
        let request: crate::coordinator::ChildRequest = serde_json::from_value(args.clone())?;
        ensure!(
            request.role != Role::Root && request.skills.len() <= 5,
            "invalid child role or skill count"
        );
        self.store
            .verify_evidence_links(&agent.run_id, &request.contract.input_evidence_ids)?;
        for skill in &request.skills {
            agents::validate_capability(skill)?;
            agents::load_skill(skill, self.config.skills_dir.as_deref())?;
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
        agent: &AgentRecord,
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
        let content = agents::load_skill(name, self.config.skills_dir.as_deref())?;
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
            let mut current: AgentRecord = Store::tx_get_metadata(tx,"agent",&agent.id)?.context("agent missing")?;
            ensure!(current.generation == agent.generation, "agent activation changed before skill load");
            agents::validate_capabilities(current.skills.iter().map(String::as_str)
                .chain(current.active_skills.iter().map(|skill| skill.name.as_str()))
                .chain(std::iter::once(name)))?;
            let snapshot = ActiveSkill { name:name.into(),sha256:sha256.clone(),content,evidence_id:evidence.id.clone() };
            if let Some(existing) = current.active_skills.iter_mut().find(|skill|skill.name==name) {
                *existing=snapshot;
            } else { current.active_skills.push(snapshot); }
            current.updated_at=now();
            Store::tx_put_metadata(tx,"agent",&agent.id,&agent.run_id,&current)?;
            Store::tx_event(tx,&agent.run_id,Some(&agent.id),"skill.activated",json!({"name":name,"sha256":sha256,"evidence_id":evidence.id}))?;
            Ok(json!({"name":name,"sha256":sha256,"evidence_id":evidence.id,"active":true,"applies":"next fresh model request"}))
        })
    }

    pub(super) fn send_message(
        &self,
        agent: &AgentRecord,
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

    pub(super) fn save_document(
        &self,
        agent: &AgentRecord,
        call_id: &str,
        name: &str,
        args: &Value,
    ) -> Result<Value> {
        let kind = match name {
            "threat_model_save" => "threat_model",
            "note_save" => "note",
            _ => "todo",
        };
        self.effect(agent, call_id, |tx| {
            let document = SharedDocument {
                id: stable("doc", call_id),
                run_id: agent.run_id.clone(),
                agent_id: agent.id.clone(),
                kind: kind.into(),
                title: text(args, "title")?.into(),
                body: args["body"].as_str().unwrap_or_default().into(),
                amendments: if kind == "todo" {
                    vec![json!({"status":"pending","agent_id":agent.id,"created_at":now()})]
                } else {
                    vec![]
                },
                created_at: now(),
                updated_at: now(),
            };
            Store::tx_put(tx, kind, &document.id, &agent.run_id, &document)?;
            Ok(serde_json::to_value(document)?)
        })
    }

    pub(super) fn amend_document(
        &self,
        agent: &AgentRecord,
        call_id: &str,
        name: &str,
        args: &Value,
    ) -> Result<Value> {
        let kind = if name == "threat_model_amend" {
            "threat_model"
        } else {
            "todo"
        };
        self.effect(agent, call_id, |tx| {
            let mut document: SharedDocument =
                Store::tx_get(tx, kind, text(args, "id")?)?.context("document missing")?;
            ensure!(
                document.run_id == agent.run_id,
                "document belongs to another run"
            );
            let mut amendment = json!({"agent_id":agent.id,"created_at":now(),"call_id":call_id});
            if name == "threat_model_amend" {
                amendment["body"] = json!(text(args, "amendment")?);
            } else {
                ensure!(
                    args.get("body").is_some() || args.get("status").is_some(),
                    "todo_update requires a body or status"
                );
                if let Some(body) = args["body"].as_str() {
                    document.body = body.into();
                    amendment["body"] = json!(body);
                }
                if let Some(status) = args["status"].as_str() {
                    amendment["status"] = json!(status);
                }
            }
            document.amendments.push(amendment);
            document.updated_at = now();
            Store::tx_put(tx, kind, &document.id, &agent.run_id, &document)?;
            Ok(serde_json::to_value(document)?)
        })
    }

    pub(super) fn coverage(
        &self,
        agent: &AgentRecord,
        call_id: &str,
        name: &str,
        args: &Value,
    ) -> Result<Value> {
        let evidence = string_list(args, "evidence_ids")?;
        self.store.verify_evidence_links(&agent.run_id, &evidence)?;
        self.effect(agent, call_id, |tx| {
            let mut coverage = if name == "coverage_update" {
                let record: Coverage = Store::tx_get(tx, "coverage", text(args, "id")?)?
                    .context("coverage missing")?;
                ensure!(
                    record.run_id == agent.run_id,
                    "coverage belongs to another run"
                );
                record
            } else {
                Coverage {
                    id: stable("cov", call_id),
                    run_id: agent.run_id.clone(),
                    agent_id: agent.id.clone(),
                    scope: text(args, "scope")?.into(),
                    kind: text(args, "kind")?.into(),
                    outcome: String::new(),
                    notes: String::new(),
                    evidence_ids: vec![],
                    created_at: now(),
                    updated_at: now(),
                }
            };
            coverage.outcome = text(args, "outcome")?.into();
            if let Some(notes) = args["notes"].as_str() {
                coverage.notes = notes.into();
            }
            if args.get("evidence_ids").is_some() {
                coverage.evidence_ids = evidence;
            }
            coverage.updated_at = now();
            Store::tx_put(tx, "coverage", &coverage.id, &agent.run_id, &coverage)?;
            Ok(serde_json::to_value(coverage)?)
        })
    }

    pub(super) fn finding(
        &self,
        agent: &AgentRecord,
        call_id: &str,
        name: &str,
        args: &Value,
    ) -> Result<Value> {
        let evidence = string_list(args, "evidence_ids")?;
        self.store.verify_evidence_links(&agent.run_id, &evidence)?;
        self.effect(agent, call_id, |tx| {
            let mut finding = if name == "finding_update" {
                let record: Finding =
                    Store::tx_get(tx, "finding", text(args, "id")?)?.context("finding missing")?;
                ensure!(
                    record.run_id == agent.run_id,
                    "finding belongs to another run"
                );
                record
            } else {
                ensure!(!evidence.is_empty(), "a finding requires original evidence");
                if let Some(record) = Store::tx_list::<Finding>(tx, "finding", &agent.run_id)?
                    .into_iter()
                    .find(|record| {
                        record.title == args["title"]
                            && record.location == args.get("location").cloned()
                    })
                {
                    return Ok(json!({"duplicate":true,"finding":record}));
                }
                Finding {
                    id: stable("finding", call_id),
                    run_id: agent.run_id.clone(),
                    agent_id: agent.id.clone(),
                    title: text(args, "title")?.into(),
                    description: text(args, "description")?.into(),
                    root_cause: None,
                    impact: None,
                    severity: None,
                    severity_rationale: None,
                    stage: FindingStage::Suspected,
                    location: None,
                    evidence_ids: vec![],
                    verification_id: None,
                    created_at: now(),
                    updated_at: now(),
                }
            };
            for key in ["title", "description"] {
                if let Some(value) = args[key].as_str() {
                    ensure!(!value.trim().is_empty(), "{key} cannot be empty");
                    if key == "title" {
                        finding.title = value.into();
                    } else {
                        finding.description = value.into();
                    }
                }
            }
            if let Some(value) = args["root_cause"].as_str() {
                finding.root_cause = Some(value.into());
            }
            if let Some(value) = args["impact"].as_str() {
                finding.impact = Some(value.into());
            }
            if let Some(value) = args.get("severity") {
                finding.severity = Some(serde_json::from_value(value.clone())?);
            }
            if let Some(value) = args["severity_rationale"].as_str() {
                finding.severity_rationale = Some(value.into());
            }
            if let Some(value) = args["verification_id"].as_str() {
                finding.verification_id = Some(value.into());
            }
            if let Some(value) = args.get("location") {
                finding.location = Some(value.clone());
            }
            if args.get("evidence_ids").is_some() {
                ensure!(!evidence.is_empty(), "a finding requires evidence");
                finding.evidence_ids = evidence;
            }
            if let Some(value) = args.get("stage") {
                finding.stage = serde_json::from_value(value.clone())?;
            }
            if let Some(severity) = finding.severity {
                ensure!(
                    !matches!(severity, Severity::Critical | Severity::High)
                        || !matches!(
                            finding.stage,
                            FindingStage::Suspected
                                | FindingStage::Inconclusive
                                | FindingStage::Rejected
                        ),
                    "severity critical/high requires a reproduced-or-better stage; \
                     a suspicion or a bare crash is at most medium"
                );
            }
            if finding.stage == FindingStage::ImpactVerified {
                let id = finding
                    .verification_id
                    .as_ref()
                    .context("impact_verified requires an independent verification_id")?;
                let verification: Verification =
                    Store::tx_get(tx, "verification", id)?.context("verification missing")?;
                ensure!(
                    verification.run_id == agent.run_id
                        && verification.finding_id == finding.id
                        && verification.verifier_agent_id != finding.agent_id
                        && verification.passed
                        && verification.attempts > 0
                        && verification.successes == verification.attempts
                        && !verification.evidence_ids.is_empty(),
                    "independent successful verification is required"
                );
                let _: AgentRecord =
                    Store::tx_get_metadata(tx, "agent", &verification.verifier_agent_id)?
                        .context("verifier agent missing")?;
                finding.impact = Some(verification.impact);
            }
            finding.updated_at = now();
            Store::tx_put(tx, "finding", &finding.id, &agent.run_id, &finding)?;
            Store::tx_event(
                tx,
                &agent.run_id,
                Some(&agent.id),
                "finding.updated",
                json!({"finding_id":finding.id,"stage":finding.stage}),
            )?;
            Ok(json!({"duplicate":false,"finding":finding}))
        })
    }
}
