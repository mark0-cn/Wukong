//! Worker IO is asynchronous and uses Rig's task/hold lifecycle.
use super::*;
use crate::tools::ToolSelection;

#[derive(Component)]
pub(super) struct PreparingTools;
#[derive(Component)]
pub(super) struct EnvironmentPreflight;
#[derive(Component)]
pub(super) struct ModelTools(pub ToolSelection);

#[expect(
    clippy::type_complexity,
    reason = "Bevy query filters express the model preparation lifecycle"
)]
pub(super) fn begin_preflights(
    calls: Query<
        (Entity, &ForAgent),
        (
            With<Admitted>,
            With<ModelRequest>,
            Without<Started>,
            Without<Outcome>,
            Without<PreparingTools>,
            Without<ModelTools>,
        ),
    >,
    owners: Query<&AgentId>,
    application: Res<Application>,
    wake: Res<Wake>,
    mut commands: Commands,
    mut fatal: ResMut<Fatal>,
) {
    for (effect, from) in &calls {
        failed(
            &mut fatal,
            (|| -> Result<()> {
                let agent = application.runner.agent(&owners.get(from.0)?.0)?;
                if !agent.predecessors_released
                    || application.runner.run(&agent.run_id)?.status.terminal()
                {
                    return Ok(());
                }
                let runner = application.runner.clone();
                let runtime = application.runtime.clone();
                let work = Running::spawn(bevy_tasks::IoTaskPool::get(), &wake, async move {
                    let task =
                        runtime.spawn(async move { runner.prepare_tool_selection(&agent).await });
                    struct Abort(tokio::task::AbortHandle);
                    impl Drop for Abort {
                        fn drop(&mut self) {
                            self.0.abort();
                        }
                    }
                    let _abort = Abort(task.abort_handle());
                    task.await
                        .map_err(|error| format!("environment preflight stopped: {error}"))
                });
                // HoldOf is a linked Rig relationship: cancelling the model also
                // drops its probe and the Tokio task, including any waiting transport.
                commands.spawn((EnvironmentPreflight, HoldOf(effect), work));
                commands.entity(effect).insert(PreparingTools);
                Ok(())
            })(),
        );
    }
}

#[expect(
    clippy::type_complexity,
    reason = "The completed probe carries its typed model selection"
)]
pub(super) fn finish_preflights(
    probes: Query<
        (Entity, &HoldOf, &Done<Result<ToolSelection, String>>),
        With<EnvironmentPreflight>,
    >,
    mut commands: Commands,
    mut fatal: ResMut<Fatal>,
) {
    for (probe, held, result) in &probes {
        match &result.0 {
            Ok(selection) => {
                commands
                    .entity(held.0)
                    .insert(ModelTools(selection.clone()));
            }
            Err(error) => {
                fatal
                    .0
                    .get_or_insert_with(|| anyhow::anyhow!(error.clone()));
            }
        }
        commands.entity(probe).despawn();
    }
}
