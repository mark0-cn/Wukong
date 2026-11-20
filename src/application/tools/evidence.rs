//! Narrow controller-owned evidence operations.
use super::*;

pub(super) struct EvidenceService {
    pub(super) store: Store,
    pub(super) output_limit: usize,
    pub(super) read_limit: usize,
}
impl EvidenceService {
    pub(super) fn effect(
        &self,
        agent: &AgentMetadata,
        call: &str,
        action: impl FnOnce(&crate::storage::StateSession<'_>) -> Result<Value>,
    ) -> Result<Value> {
        DurableEffects::new(&self.store, self.output_limit).commit(agent, call, action)
    }
    pub(super) fn coverage(
        &self,
        agent: &AgentMetadata,
        call_id: &str,
        args: &Value,
    ) -> Result<Value> {
        let evidence = string_list(args, "evidence_ids")?;
        ensure!(
            evidence.len() <= 128,
            "evidence links are limited to 128 per record"
        );
        self.store.verify_evidence_links(&agent.run_id, &evidence)?;
        let scope = text(args, "scope")?
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let kind = text(args, "kind")?
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let identity = stable(
            "cov",
            &serde_json::to_string(&(&agent.run_id, &agent.id, &scope, &kind))?,
        );
        self.effect(agent, call_id, |tx| {
            let mut matching = tx
                .records::<Coverage>(&agent.run_id)?
                .into_iter()
                .filter(|record| {
                    record.agent_id == agent.id
                        && record
                            .scope
                            .split_whitespace()
                            .collect::<Vec<_>>()
                            .join(" ")
                            == scope
                        && record.kind.split_whitespace().collect::<Vec<_>>().join(" ") == kind
                })
                .collect::<Vec<_>>();
            matching.sort_by(|left, right| {
                (left.id != identity, &left.created_at, &left.id).cmp(&(
                    right.id != identity,
                    &right.created_at,
                    &right.id,
                ))
            });
            let mut coverage = if matching.is_empty() {
                Coverage {
                    id: identity.clone(),
                    run_id: agent.run_id.clone(),
                    agent_id: agent.id.clone(),
                    scope: scope.clone(),
                    kind: kind.clone(),
                    outcome: String::new(),
                    notes: String::new(),
                    evidence_ids: vec![],
                    created_at: now(),
                    updated_at: now(),
                }
            } else {
                matching.remove(0)
            };
            coverage.scope = scope.clone();
            coverage.kind = kind.clone();
            coverage.outcome = text(args, "outcome")?.into();
            if let Some(notes) = args["notes"].as_str() {
                coverage.notes = notes.into();
            }
            if args.get("evidence_ids").is_some() {
                coverage.evidence_ids = evidence;
            }
            coverage.updated_at = now();
            tx.save_record(&coverage)?;
            for mut duplicate in matching {
                duplicate.outcome = coverage.outcome.clone();
                duplicate.updated_at = coverage.updated_at.clone();
                tx.save_record(&duplicate)?;
            }
            Ok(serde_json::to_value(coverage)?)
        })
    }

    pub(super) fn finding(
        &self,
        agent: &AgentMetadata,
        call_id: &str,
        name: &str,
        args: &Value,
    ) -> Result<Value> {
        let evidence = string_list(args, "evidence_ids")?;
        ensure!(
            evidence.len() <= 128,
            "evidence links are limited to 128 per record"
        );
        self.store.verify_evidence_links(&agent.run_id, &evidence)?;
        self.effect(agent, call_id, |tx| {
            let mut finding = if name == "finding_update" {
                let record: Finding = tx
                    .get_record(text(args, "id")?)?
                    .context("finding missing")?;
                ensure!(
                    record.run_id == agent.run_id,
                    "finding belongs to another run"
                );
                record
            } else {
                ensure!(!evidence.is_empty(), "a finding requires original evidence");
                if let Some(record) =
                    tx.records::<Finding>(&agent.run_id)?
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
                    tx.get_record(id)?.context("verification missing")?;
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
                let verifier: AgentMetadata = tx
                    .get_record::<AgentMetadata>(&verification.verifier_agent_id)?
                    .context("verifier agent missing")?;
                ensure!(
                    verifier.role == Role::Verifier && verifier.run_id == agent.run_id,
                    "verification requires a same-run verifier role"
                );
                finding.impact = Some(verification.impact);
            }
            finding.updated_at = now();
            tx.save_record(&finding)?;
            tx.event(
                &agent.run_id,
                Some(&agent.id),
                "finding.updated",
                json!({"finding_id":finding.id,"stage":finding.stage}),
            )?;
            Ok(json!({"duplicate":false,"finding":finding}))
        })
    }
}
