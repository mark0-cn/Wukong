//! Narrow application operations available to the Rig adapter.
use crate::{
    agents,
    config::{AppConfig, ExecutionConfig},
    coordinator::Coordinator,
    domain::*,
    llm::ModelEngine,
    model_catalog::ModelCatalog,
    store::Store,
    tools::{ToolOutcome, ToolService},
    usage::UsageService,
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc};
use tokio::sync::broadcast;
#[derive(Clone)]
pub(crate) struct KernelHost {
    store: Store,
    tools: Arc<ToolService>,
    config: AppConfig,
    models: Arc<ModelCatalog>,
    usage: UsageService,
    events: broadcast::Sender<Value>,
    coordinator: Coordinator,
}
impl KernelHost {
    pub(crate) fn publish_reply(
        &self,
        reply: &PublicReply,
        text: &str,
        state: ReplyState,
    ) -> Result<()> {
        for notice in crate::progress::publish(&self.store, reply, text, state)? {
            let _ = self.events.send(notice);
        }
        Ok(())
    }
    pub(crate) fn new(
        store: Store,
        tools: Arc<ToolService>,
        config: AppConfig,
        models: Arc<ModelCatalog>,
        usage: UsageService,
        events: broadcast::Sender<Value>,
    ) -> Self {
        Self {
            coordinator: Coordinator::new(store.clone()),
            store,
            tools,
            config,
            models,
            usage,
            events,
        }
    }
    pub(crate) fn execution(&self) -> &ExecutionConfig {
        &self.config.execution
    }
    pub(crate) fn engine_for(&self, role: Role) -> &ModelEngine {
        self.models.engine_for(role)
    }
    pub(crate) fn role_config(&self, role: Role) -> crate::llm::ModelConfig {
        self.models.config().role_config_for(role)
    }
    pub(crate) fn agent(&self, id: &str) -> Result<AgentMetadata> {
        self.store.load::<AgentMetadata>(id)
    }
    pub(crate) fn maybe_agent(&self, id: &str) -> Result<Option<AgentMetadata>> {
        self.store.get_record::<AgentMetadata>(id)
    }
    pub(crate) fn agents(&self, run: &str) -> Result<Vec<AgentMetadata>> {
        self.store.records::<AgentMetadata>(run)
    }
    pub(crate) fn run(&self, id: &str) -> Result<RunRecord> {
        self.store.load(id)
    }
    pub(crate) fn journal_dir(&self, run: &str) -> PathBuf {
        self.config.state_dir.join("rig").join(run)
    }
    pub(crate) fn prepare_run(&self, run_id: &str) -> Result<RunRecord> {
        let mut run = self.run(run_id)?;
        ensure!(!run.status.terminal(), "cannot drive a terminal assessment");
        self.coordinator.resume(run_id, None)?;
        let root = self.store.agent_snapshot(&run.root_id)?;
        ensure!(
            root.checkpoint.is_none()
                || self
                    .journal_dir(run_id)
                    .join("logs")
                    .join(format!("{}.jsonl", run.root_id))
                    .is_file(),
            "this run has no native Rig journal; start a new assessment"
        );
        if root.checkpoint.is_none() {
            run.config = serde_json::to_value(&self.config)?;
            self.store.save_record(&run)?;
        } else {
            let saved: AppConfig = serde_json::from_value(run.config.clone())?;
            ensure!(
                serde_json::to_value(saved.model)? == serde_json::to_value(&self.config.model)?,
                "native session model configuration changed; use the saved provider/endpoint configuration"
            );
        }
        Ok(run)
    }
    pub(crate) fn preamble(&self, agent: &AgentMetadata, run: &RunRecord) -> Result<String> {
        let mut text = agents::preamble(
            agent,
            run,
            &self.store,
            self.config.skills_dir.as_deref(),
            self.config.demos_dir.as_deref(),
        )?;
        for skill in &agent.active_skills {
            agents::validate_capability(&skill.name)?;
            ensure!(
                crate::binary::sha256_bytes(skill.content.as_bytes()) == skill.sha256,
                "active skill {} failed its content hash check",
                skill.name
            );
            text.push_str(&format!(
                "\n\n# Active controller skill: {} (SHA256 {})\n{}",
                skill.name, skill.sha256, skill.content
            ));
        }
        Ok(text)
    }
    pub(crate) fn definitions(&self, role: Role) -> Vec<rig::completion::ToolDefinition> {
        self.tools.definitions(role)
    }
    pub(crate) fn allows(&self, role: Role, name: &str) -> bool {
        self.tools.model_allows(role, name)
    }
    pub(crate) fn tool_names(
        &self,
        agent: &AgentMetadata,
    ) -> Result<std::collections::BTreeSet<String>> {
        self.tools.model_tool_names(agent)
    }
    pub(crate) async fn prepare_tool_selection(
        &self,
        agent: &AgentMetadata,
    ) -> crate::tools::ToolSelection {
        self.tools.prepare_tool_selection(agent).await
    }
    pub(crate) fn create_agent(
        &self,
        agent: &AgentMetadata,
        call: &str,
        args: &Value,
    ) -> Result<Value> {
        self.tools.create_agent(agent, call, args)
    }
    pub(crate) async fn execute(
        &self,
        agent: &str,
        call: &str,
        name: &str,
        args: Value,
    ) -> Result<ToolOutcome> {
        self.tools.execute(agent, call, name, args).await
    }
    pub(crate) fn replace_call_result(
        &self,
        call: &str,
        value: &Value,
        is_error: bool,
    ) -> Result<()> {
        self.tools.replace_call_result(call, value, is_error)
    }
    pub(crate) fn subscribe_changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.store.subscribe_changes()
    }
    pub(crate) fn inbox(&self, id: &str) -> Result<Vec<Message>> {
        self.store.inbox(id)
    }
    pub(crate) fn checkpoint_and_ack(
        &self,
        id: &str,
        checkpoint: Value,
        messages: &[i64],
    ) -> Result<()> {
        self.store.checkpoint_and_ack(id, checkpoint, messages)
    }
    pub(crate) fn checkpoint_and_ack_generation(
        &self,
        id: &str,
        checkpoint: Value,
        messages: &[i64],
        generation: Option<u64>,
    ) -> Result<()> {
        self.store
            .checkpoint_and_ack_generation(id, checkpoint, messages, generation)
    }
    pub(crate) fn pause_budget(&self, run: &str, reason: &str) -> Result<()> {
        self.coordinator.pause_budget(run, reason)
    }
    pub(crate) fn release_dependencies(&self, run: &str) -> Result<usize> {
        self.coordinator.release_dependencies(run)
    }
    pub(crate) fn wake_waiters(&self, run: &str) -> Result<usize> {
        self.coordinator.wake_waiters(run)
    }
    pub(crate) fn set_status(
        &self,
        id: &str,
        status: Status,
        reason: Option<String>,
    ) -> Result<()> {
        self.coordinator.set_status(id, status, reason)
    }
    pub(crate) fn account_usage(
        &self,
        id: &str,
        usage: rig::completion::Usage,
        turn: bool,
    ) -> Result<()> {
        self.usage.account_usage(id, usage, turn)
    }
    pub(crate) fn checkpoint_evidence(
        &self,
        run: &str,
        agent: &str,
        call: &str,
        bytes: &[u8],
    ) -> Result<Evidence> {
        self.store.artifact(
            run,
            agent,
            call,
            "context.checkpoint",
            bytes,
            "application/json",
            json!({"native_rig_conversation":true}),
        )
    }
    pub(crate) fn fail_assessment(&self, mut run: RunRecord, reason: &str) -> Result<()> {
        run.status = Status::Failed;
        run.summary = Some(reason.into());
        run.updated_at = now();
        self.store.save_record(&run)
    }
    pub(crate) fn stream_progress(&self, run_id: &str, agent_id: &str) {
        let _ = self
            .events
            .send(json!({"run_id":run_id,"agent_id":agent_id,"kind":"model.stream_progress"}));
    }
    pub(crate) fn emit(
        &self,
        run_id: &str,
        agent_id: Option<&str>,
        kind: &str,
        data: Value,
    ) -> Result<()> {
        let seq = self.store.event(run_id, agent_id, kind, data.clone())?;
        let _ = self
            .events
            .send(json!({"seq":seq,"run_id":run_id,"agent_id":agent_id,"kind":kind,"data":data}));
        Ok(())
    }
    pub(crate) fn fail_agent(
        &self,
        agent_id: &str,
        reason: String,
        summary: Option<String>,
    ) -> Result<()> {
        let run_id = self.store.write(|tx| {
            let mut target: AgentMetadata = tx
                .get_record::<AgentMetadata>(agent_id)?
                .context("agent missing")?;
            if target.status.terminal() {
                return Ok(None);
            }
            target.status = Status::Failed;
            target.error = Some(reason.clone());
            if let Some(summary) = &summary {
                target.summary = Some(summary.clone());
            }
            target.updated_at = now();
            tx.save_record(&target)?;
            Coordinator::notify_terminal(tx, &target)?;
            Ok(Some(target.run_id))
        })?;
        if let Some(run_id) = run_id {
            self.emit(
                &run_id,
                Some(agent_id),
                "agent.error",
                json!({"message":reason}),
            )?;
        }
        Ok(())
    }

    pub(crate) fn structured_summary(
        &self,
        agent: &AgentMetadata,
        run: &RunRecord,
        checkpoint_evidence: &str,
    ) -> Result<String> {
        self.store.read(|tx| {
            Ok(serde_json::to_string(&json!({
                "notice":"Shared analysis checkpoint; retrieve the older transcript with evidence_read. Shared context is refreshed on each request. Use context_search/context_read, view_agent_graph, findings_list and coverage_list for omitted ledger entries. Preserve unknowns.",
                "full_checkpoint_evidence_id":checkpoint_evidence,
                "task":agent.task,"scope":agent.scope,"target_sha256":run.target_sha256,
                "shared_context":crate::context::snapshot(tx,agent)?,
                "ledger_counts":{
                    "tasks":tx.records::<AgentMetadata>(&run.id)?.len(),
                    "findings":tx.records::<Finding>(&run.id)?.len(),
                    "coverage":tx.records::<Coverage>(&run.id)?.len()
                }
            }))?)
        })
    }
}
