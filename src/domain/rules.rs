//! Pure task, dependency, completion and mailbox policies.
use super::*;
use anyhow::{Context, Result, ensure};
use std::collections::HashSet;

pub struct TaskGraph;
impl TaskGraph {
    pub fn validate(agents: &[AgentMetadata]) -> Result<()> {
        fn visit<'a>(
            id: &'a str,
            agents: &'a [AgentMetadata],
            visiting: &mut HashSet<&'a str>,
            visited: &mut HashSet<&'a str>,
        ) -> Result<()> {
            if visited.contains(id) {
                return Ok(());
            }
            ensure!(
                visiting.insert(id),
                "agent dependencies would deadlock (cycle at {id})"
            );
            if let Some(agent) = agents.iter().find(|a| a.id == id && !a.status.terminal()) {
                for dependency in agents.iter().filter(|other| {
                    !other.status.terminal()
                        && (other.parent_id.as_deref() == Some(id)
                            || (!agent.predecessors_released
                                && agent.wait_for_predecessors.contains(&other.id))
                            || agent.wait_for.contains(&other.id))
                }) {
                    visit(&dependency.id, agents, visiting, visited)?;
                }
            }
            visiting.remove(id);
            visited.insert(id);
            Ok(())
        }
        let mut visiting = HashSet::new();
        let mut visited = HashSet::new();
        for agent in agents.iter().filter(|a| !a.status.terminal()) {
            visit(&agent.id, agents, &mut visiting, &mut visited)?;
        }
        Ok(())
    }
    pub fn is_descendant(agent: &AgentMetadata, parent: &str, all: &[AgentMetadata]) -> bool {
        let mut current = agent.parent_id.as_deref();
        let mut traversed = 0;
        while let Some(id) = current {
            if id == parent {
                return true;
            }
            current = all
                .iter()
                .find(|a| a.id == id)
                .and_then(|a| a.parent_id.as_deref());
            traversed += 1;
            if traversed > all.len() {
                break;
            }
        }
        false
    }
}
pub struct DependencyRules;
impl DependencyRules {
    pub fn evaluate(
        policy: DependencyPolicy,
        targets: &[String],
        all: &[AgentMetadata],
    ) -> DependencyState {
        if targets.is_empty() {
            return DependencyState::Satisfied;
        }
        let mut terminal = 0;
        let mut successes = 0;
        for target in targets {
            let Some(agent) = all.iter().find(|agent| &agent.id == target) else {
                return DependencyState::Blocked {
                    reason: format!("dependency {target} is missing"),
                };
            };
            let success = agent.status == Status::Completed
                && agent
                    .task_result
                    .as_ref()
                    .is_some_and(|result| result.outcome == TaskOutcome::Success);
            if success {
                successes += 1;
            }
            if agent.status.terminal() {
                terminal += 1;
            }
            if policy == DependencyPolicy::AllSuccess && agent.status.terminal() && !success {
                return DependencyState::Blocked {
                    reason: format!(
                        "dependency {target} ended without success ({})",
                        agent.status
                    ),
                };
            }
        }
        match policy {
            DependencyPolicy::AllSuccess if successes == targets.len() => {
                DependencyState::Satisfied
            }
            DependencyPolicy::AllTerminal if terminal == targets.len() => {
                DependencyState::Satisfied
            }
            DependencyPolicy::AnySuccess if successes > 0 => DependencyState::Satisfied,
            DependencyPolicy::AnySuccess if terminal == targets.len() => DependencyState::Blocked {
                reason: "all dependency alternatives ended without success".into(),
            },
            _ => DependencyState::Pending,
        }
    }
}
pub struct CompletionRules;
impl CompletionRules {
    pub fn validate_contract(contract: &TaskContract) -> Result<()> {
        ensure!(
            !contract.objective.trim().is_empty() && contract.objective.len() <= 65536,
            "invalid contract objective"
        );
        for values in [
            &contract.constraints,
            &contract.deliverables,
            &contract.completion_conditions,
        ] {
            ensure!(
                values.len() <= 32
                    && values
                        .iter()
                        .all(|value| !value.trim().is_empty() && value.len() <= 4096),
                "invalid contract requirement"
            );
            ensure!(
                values.iter().collect::<HashSet<_>>().len() == values.len(),
                "duplicate contract requirement"
            );
        }
        Ok(())
    }
    pub fn validate_result(agent: &AgentMetadata, result: &TaskResult) -> Result<()> {
        ensure!(
            !result.answer.trim().is_empty() && result.answer.len() <= 65536,
            "invalid result answer"
        );
        ensure!(result.artifacts.len() <= 32, "too many task artifacts");
        let names: HashSet<_> = result
            .artifacts
            .iter()
            .map(|artifact| artifact.name.as_str())
            .collect();
        ensure!(
            names.len() == result.artifacts.len()
                && names
                    .iter()
                    .all(|name| !name.trim().is_empty() && name.len() <= 4096),
            "invalid or duplicate artifact name"
        );
        for values in [
            &result.completed_conditions,
            &result.unknowns,
            &result.next_actions,
        ] {
            ensure!(
                values.len() <= 32
                    && values
                        .iter()
                        .all(|value| !value.trim().is_empty() && value.len() <= 4096),
                "invalid task result detail"
            );
        }
        ensure!(
            result
                .completed_conditions
                .iter()
                .all(|condition| agent.contract.completion_conditions.contains(condition)),
            "result claims an undeclared completion condition"
        );
        if result.outcome == TaskOutcome::Success {
            ensure!(
                agent
                    .contract
                    .deliverables
                    .iter()
                    .all(|name| names.contains(name.as_str())),
                "required task deliverables missing; provide structured result artifacts"
            );
            ensure!(
                agent
                    .contract
                    .completion_conditions
                    .iter()
                    .all(|condition| result.completed_conditions.contains(condition)),
                "required completion conditions not accounted for"
            );
        } else {
            ensure!(
                !result.unknowns.is_empty() || !result.next_actions.is_empty(),
                "non-success result requires unknowns or next_actions"
            );
        }
        Ok(())
    }
}
pub struct MessageRules;
impl MessageRules {
    pub fn prepare(
        sender: &str,
        target: &AgentMetadata,
        sender_generation: Option<u64>,
        content: &str,
        options: &mut MessageOptions,
        original: Option<&Message>,
    ) -> Result<MessageEnvelope> {
        ensure!(
            !content.trim().is_empty() && content.len() <= 65536,
            "message must be nonempty and at most 64 KiB"
        );
        ensure!(
            options.evidence_ids.len() <= 32,
            "at most 32 evidence references per message"
        );
        let mut stale = options
            .recipient_generation
            .is_some_and(|value| value != target.generation);
        if options.kind == MessageKind::Reply {
            let original = original.context("reply target missing")?;
            ensure!(
                original.envelope.options.kind == MessageKind::Question
                    && original.sender == target.id
                    && original.recipient == sender,
                "reply must answer a question addressed to this sender"
            );
            stale |= original
                .envelope
                .sender_generation
                .is_some_and(|generation| generation != target.generation)
                || original.envelope.options.recipient_generation != sender_generation;
            if let Some(correlation) = &options.correlation_id {
                ensure!(
                    Some(correlation) == original.envelope.options.correlation_id.as_ref(),
                    "reply correlation mismatch"
                );
            }
            options.correlation_id = original.envelope.options.correlation_id.clone();
        } else {
            ensure!(
                options.reply_to.is_none(),
                "reply_to is only valid for replies"
            );
        };
        let correlation = options
            .correlation_id
            .get_or_insert_with(|| id("conversation"))
            .clone();
        ensure!(
            !correlation.trim().is_empty() && correlation.len() <= 256,
            "invalid correlation_id"
        );
        let quota_failed = target.status == Status::Failed
            && target
                .error
                .as_deref()
                .is_some_and(|error| error.contains("budget"));
        let resumable = target.status == Status::Completed || quota_failed;
        let generation = if options.kind == MessageKind::FollowUp
            && resumable
            && !stale
        {
            target
                .generation
                .checked_add(1)
                .context("agent generation exhausted")?
        } else {
            options.recipient_generation.unwrap_or(target.generation)
        };
        options.recipient_generation = Some(generation);
        Ok(MessageEnvelope {
            options: options.clone(),
            task_id: target.id.clone(),
            sender_generation,
            delivery: if stale {
                MessageDelivery::Stale
            } else {
                MessageDelivery::Queued
            },
            stale_reason: stale.then(|| "message belongs to a different task generation".into()),
            ..Default::default()
        })
    }
}

#[derive(Default)]
pub struct CompletionFacts {
    pub unread_messages: i64,
    pub agents: Vec<AgentMetadata>,
    pub open_sessions: Vec<(String, String, String)>,
    pub unresolved_coverage: Vec<String>,
    pub running_jobs: Vec<String>,
}
impl CompletionRules {
    pub fn check_ready(agent: &AgentMetadata, facts: &CompletionFacts) -> Result<()> {
        ensure!(
            facts.unread_messages == 0,
            "unread coordinator messages remain; read them at the next model boundary before finishing"
        );
        let active = facts
            .agents
            .iter()
            .filter(|other| {
                !other.status.terminal()
                    && TaskGraph::is_descendant(other, &agent.id, &facts.agents)
            })
            .map(|other| other.id.as_str())
            .collect::<Vec<_>>();
        ensure!(
            active.is_empty(),
            "active descendants remain: {}",
            active.join(", ")
        );
        for kind in ["debug_session", "interact_session"] {
            let open = facts
                .open_sessions
                .iter()
                .filter(|(category, _, owner)| {
                    category == kind && (agent.role == Role::Root || owner == &agent.id)
                })
                .map(|(_, id, owner)| format!("{id} (owner {owner})"))
                .collect::<Vec<_>>();
            ensure!(
                open.is_empty(),
                "open {kind} sessions remain; close them before finishing: {}",
                open.join(", ")
            );
        }
        if agent.role == Role::Root {
            ensure!(
                facts.unresolved_coverage.is_empty(),
                "coverage needs follow-up; resolve or mark inconclusive: {}",
                facts.unresolved_coverage.join(", ")
            );
            ensure!(
                facts.running_jobs.is_empty(),
                "running worker jobs remain; wait for or cancel them before finish_scan: {}",
                facts.running_jobs.join(", ")
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn agent(id: &str, parent: Option<&str>) -> AgentMetadata {
        AgentMetadata {
            id: id.into(),
            run_id: "run".into(),
            parent_id: parent.map(str::to_owned),
            name: id.into(),
            role: if parent.is_none() {
                Role::Root
            } else {
                Role::Analyst
            },
            task: "fixture".into(),
            scope: id.into(),
            contract: TaskContract::default(),
            task_result: None,
            skills: vec![],
            active_skills: vec![],
            status: Status::Running,
            depth: usize::from(parent.is_some()) as u32,
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
            token_cap: None,
            summary: None,
            error: None,
            created_at: now(),
            updated_at: now(),
        }
    }
    #[test]
    fn task_cycles_include_parent_completion_edges() {
        let root = agent("root", None);
        let mut child = agent("child", Some("root"));
        assert!(TaskGraph::validate(&[root.clone(), child.clone()]).is_ok());
        child.wait_for_predecessors.push(root.id.clone());
        child.predecessors_released = false;
        assert!(TaskGraph::validate(&[root, child]).is_err());
    }
    #[test]
    fn completion_policy_checks_task_and_assessment_obligations() {
        let root = agent("root", None);
        let mut child = agent("child", Some("root"));
        let mut facts = CompletionFacts {
            agents: vec![root.clone(), child.clone()],
            ..Default::default()
        };
        assert!(CompletionRules::check_ready(&root, &facts).is_err());
        child.status = Status::Completed;
        facts.agents[1] = child;
        facts.running_jobs.push("job".into());
        assert!(CompletionRules::check_ready(&root, &facts).is_err());
        facts.running_jobs.clear();
        assert!(CompletionRules::check_ready(&root, &facts).is_ok());
        facts.unread_messages = 1;
        assert!(CompletionRules::check_ready(&root, &facts).is_err());
    }
    #[test]
    fn stale_followups_cannot_advance_generation() -> Result<()> {
        let mut target = agent("child", Some("root"));
        target.status = Status::Completed;
        target.generation = 2;
        let mut options = MessageOptions {
            kind: MessageKind::FollowUp,
            recipient_generation: Some(1),
            ..Default::default()
        };
        assert_eq!(
            MessageRules::prepare("operator", &target, None, "next", &mut options, None)?.delivery,
            MessageDelivery::Stale
        );
        assert_eq!(options.recipient_generation, Some(1));
        options.recipient_generation = Some(2);
        assert_eq!(
            MessageRules::prepare("operator", &target, None, "next", &mut options, None)?.delivery,
            MessageDelivery::Queued
        );
        assert_eq!(options.recipient_generation, Some(3));
        Ok(())
    }
}
