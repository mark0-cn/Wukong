//! Environment configuration and identity checks, independent of tool routing.
use super::*;
pub(super) struct EnvironmentService {
    pub(super) store: Store,
    pub(super) registry: crate::adapters::environments::EnvironmentRegistry,
}
impl EnvironmentService {
    pub(super) async fn checked(&self, agent: &AgentMetadata) -> Result<RuntimeClient> {
        let environment_id = agent
            .environment_id
            .as_ref()
            .context("task has no assigned environment")?;
        self.checked_id(agent, environment_id).await
    }

    pub(super) async fn checked_id(
        &self,
        agent: &AgentMetadata,
        environment_id: &str,
    ) -> Result<RuntimeClient> {
        let config = self.registry.config(environment_id)?;
        let run: RunRecord = self.store.load(&agent.run_id)?;
        ensure!(
            run.config["environments"][environment_id] == serde_json::to_value(config)?,
            "environment configuration changed since this assessment was created"
        );
        let client = self.registry.client(environment_id).await?;
        self.store.bind_environment(&EnvironmentBinding {
            id: stable(
                "environment-binding",
                &format!("{}:{environment_id}", agent.run_id),
            ),
            run_id: agent.run_id.clone(),
            environment_id: environment_id.to_owned(),
            identity: client.identity().clone(),
        })?;
        Ok(client)
    }
}
