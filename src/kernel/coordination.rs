//! Coordination systems for the Rig adapter.
use super::*;

pub(super) fn project_conversations(
    agents: Query<(&AgentId, &Conversation), Changed<Conversation>>,
    application: Res<Application>,
    mut fatal: ResMut<Fatal>,
) {
    for (id, conversation) in &agents {
        if application
            .runner
            .store
            .get_metadata("agent", &id.0)
            .ok()
            .flatten()
            .is_some()
        {
            failed(
                &mut fatal,
                application.runner.store.checkpoint_and_ack(
                    &id.0,
                    serde_json::to_value(&conversation.0).unwrap(),
                    &[],
                ),
            );
        }
    }
}

pub(super) fn deliver_mail(
    app: &mut App,
    runner: &AssessmentRunner,
    run_id: &str,
    included: &mut HashSet<i64>,
) -> Result<()> {
    let agents: HashMap<String, Entity> = app
        .world_mut()
        .query::<(Entity, &AgentId)>()
        .iter(app.world())
        .map(|(entity, id)| (id.0.clone(), entity))
        .collect();
    for agent in runner.store.list_metadata("agent", run_id)? {
        let Some(&entity) = agents.get(&agent.id) else {
            continue;
        };
        // Completed work keeps queued mail until an explicit follow-up. The
        // coordinator then marks older-generation mail stale before delivery.
        if agent.status.terminal() {
            continue;
        }
        for mail in runner
            .store
            .inbox(&agent.id)?
            .into_iter()
            .filter(|mail| !included.contains(&mail.id))
            .collect::<Vec<_>>()
        {
            let followup = mail.envelope.options.kind == domain::MessageKind::FollowUp;
            let native_report = serde_json::from_str::<Value>(&mail.content)
                .ok()
                .is_some_and(|value| value["type"] == "agent_finished")
                && agents.contains_key(&mail.sender)
                && runner
                    .store
                    .require_metadata("agent", &mail.sender)
                    .is_ok_and(|sender| Some(sender.generation) == mail.envelope.sender_generation);
            // The native Report carries this ledger notification. Do not send
            // a second message that can race its parent's next model call.
            if native_report {
                continue;
            }
            let informational = mail.envelope.options.kind == domain::MessageKind::Information;
            // Rig's wait observes messages attributed to the awaited agent,
            // including Inform messages. A typed information notice is host
            // data carrying its sender in the envelope, never a request/report.
            let author = if informational {
                Author::plugin("coordination-information")
            } else if mail.sender == "operator" {
                Author::user()
            } else {
                Author::agent(
                    AgentId(mail.sender.clone()),
                    followup.then(|| RequestId(format!("message-{}", mail.id))),
                )
            };
            if followup && let Some(&sender) = agents.get(&mail.sender) {
                let request = Request {
                    id: RequestId(format!("message-{}", mail.id)),
                    asker: AgentId(mail.sender.clone()),
                };
                let mut current = app
                    .world()
                    .get::<OpenRequests>(entity)
                    .cloned()
                    .unwrap_or_default();
                current.0.push(request);
                app.world_mut().entity_mut(entity).insert(current);
                let _ = sender;
            }
            let mode = if !informational
                && !agent.status.terminal()
                && (followup
                    || mail.sender == "operator"
                    || mail.envelope.options.kind == domain::MessageKind::Result)
            {
                DeliveryMode::Steer
            } else {
                DeliveryMode::Inform
            };
            app.world_mut().spawn((
                Incoming::new(serde_json::to_string(&mail)?, mode).with_author(author),
                ForAgent(entity),
                MailDelivery {
                    id: mail.id,
                    generation: agent.generation,
                },
            ));
            included.insert(mail.id);
        }
    }
    Ok(())
}

pub(super) fn sync_statuses(app: &mut App, runner: &AssessmentRunner, run_id: &str) -> Result<()> {
    let records: HashMap<_, _> = runner
        .store
        .list_metadata("agent", run_id)?
        .into_iter()
        .map(|agent| (agent.id.clone(), agent))
        .collect();
    let stopped: Vec<_> = app
        .world_mut()
        .query::<(Entity, &AgentId)>()
        .iter(app.world())
        .filter(|(_, id)| {
            records
                .get(&id.0)
                .is_some_and(|agent| matches!(agent.status, Status::Stopped | Status::Failed))
        })
        .map(|(entity, _)| entity)
        .collect();
    for entity in stopped {
        if app.world().get::<ActiveTurn>(entity).is_some() {
            app.world_mut().trigger(StopTurn::new(entity));
        }
    }
    Ok(())
}

pub(super) fn carry_native_turns(
    turns: Query<Entity, (With<Restored>, With<TurnOf>)>,
    mut commands: Commands,
) {
    for turn in &turns {
        commands.entity(turn).insert(CarryOn);
    }
}

#[expect(
    clippy::type_complexity,
    reason = "Keep Bevy query components and filters visible in system signatures"
)]
pub(super) fn acknowledge_mail(
    deliveries: Query<(&MailDelivery, &ForAgent), Or<(With<DeliveredIn>, With<Settled>)>>,
    agents: Query<(&AgentId, &Conversation)>,
    application: Res<Application>,
    mut fatal: ResMut<Fatal>,
) {
    for (mail, from) in &deliveries {
        failed(
            &mut fatal,
            (|| -> Result<()> {
                let (id, conversation) = agents.get(from.0)?;
                application.runner.store.checkpoint_and_ack_generation(
                    &id.0,
                    serde_json::to_value(&conversation.0)?,
                    &[mail.id],
                    Some(mail.generation),
                )
            })(),
        );
    }
}

pub(super) fn inform_terminal(
    mut incoming: Query<(&mut Incoming, &ForAgent), Added<Admitted>>,
    ids: Query<&AgentId>,
    application: Res<Application>,
) {
    for (mut message, from) in &mut incoming {
        if let Ok(id) = ids.get(from.0)
            && application
                .runner
                .store
                .require_metadata("agent", &id.0)
                .is_ok_and(|agent| agent.status.terminal())
        {
            message.mode = DeliveryMode::Inform;
        }
    }
}

#[expect(
    clippy::type_complexity,
    reason = "Keep Bevy query components and filters visible in system signatures"
)]
pub(super) fn bind_reports(
    mut reports: Query<(Entity, &ForAgent, &mut Incoming), (With<Report>, Added<Admitted>)>,
    ids: Query<&AgentId>,
    application: Res<Application>,
    mut commands: Commands,
    mut fatal: ResMut<Fatal>,
) {
    for (entity, from, mut report) in &mut reports {
        failed(
            &mut fatal,
            (|| -> Result<()> {
                let recipient = ids.get(from.0)?;
                let Some(sender) = report.author.from.as_deref() else {
                    return Ok(());
                };
                let owner: AgentMetadata = application
                    .runner
                    .store
                    .require_metadata("agent", &recipient.0)?;
                let source: AgentMetadata =
                    application.runner.store.require_metadata("agent", sender)?;
                if let Some(mail) = application
                    .runner
                    .store
                    .inbox(&recipient.0)?
                    .into_iter()
                    .find(|mail| {
                        mail.sender == sender
                            && mail.envelope.sender_generation == Some(source.generation)
                            && serde_json::from_str::<Value>(&mail.content)
                                .is_ok_and(|value| value["type"] == "agent_finished")
                    })
                {
                    *report.text_mut() = mail.content;
                    commands.entity(entity).insert(MailDelivery {
                        id: mail.id,
                        generation: owner.generation,
                    });
                }
                Ok(())
            })(),
        );
    }
}

/// A reactivated business task still owes its tree owner a result, even
/// when an ancestor assigned the follow-up directly to a deeper descendant.
pub(super) fn ensure_owner_requests(
    mut agents: Query<(&AgentId, &mut OpenRequests), With<BusinessTask>>,
    application: Res<Application>,
    mut fatal: ResMut<Fatal>,
) {
    for (id, mut open) in &mut agents {
        failed(
            &mut fatal,
            (|| -> Result<()> {
                let task: AgentMetadata =
                    application.runner.store.require_metadata("agent", &id.0)?;
                if !task.status.terminal()
                    && let Some(owner) = task.parent_id
                    && !open.0.iter().any(|request| request.asker.0 == owner)
                {
                    open.0.push(Request {
                        id: RequestId(format!("owner:{}:{}", task.id, task.generation)),
                        asker: AgentId(owner),
                    });
                }
                Ok(())
            })(),
        );
    }
}
