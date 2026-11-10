//! Application commands own execution and configuration; the webview only supplies intent.
use super::{
    configuration::{ConfigDocument, ConfigEdit},
    workspace::{self, WorkspaceRequest, WorkspaceResponse},
};
use crate::{
    application::AssessmentService,
    config::AppConfig,
    domain::*,
    queries::{FindingDetails, FindingQueries},
    report,
    store::Store,
};
use anyhow::{Result, ensure};
use serde::Serialize;
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
#[derive(Clone)]
struct ContextState {
    epoch: u64,
    config: AppConfig,
    store: Store,
    path: PathBuf,
    state_override: Option<PathBuf>,
    selected: Option<String>,
}
#[derive(Clone, Serialize)]
pub struct ExecutionInfo {
    pub operation: String,
    pub run_id: Option<String>,
    pub phase: &'static str,
}
struct Active {
    info: ExecutionInfo,
    cancel: CancellationToken,
}
#[derive(Clone, Serialize)]
pub struct Notice {
    pub id: u64,
    pub error: bool,
    pub message: String,
}
#[derive(Serialize)]
pub struct HostStatus {
    pub epoch: u64,
    pub initial_run: Option<String>,
    pub config_path: PathBuf,
    pub state_dir: PathBuf,
    pub model: String,
    pub budget: Budget,
    pub execution: Option<ExecutionInfo>,
    pub pending_operations: usize,
    pub notice: Option<Notice>,
}
#[derive(Serialize)]
pub struct WorkspaceEnvelope {
    pub host: HostStatus,
    pub workspace: WorkspaceResponse,
}
#[derive(Serialize)]
pub struct EvidenceView {
    pub id: String,
    pub source: String,
    pub media_type: String,
    pub sha256: String,
    pub size: u64,
    pub offset: u64,
    pub end: u64,
    pub text: String,
    pub binary: bool,
    pub agent_id: String,
    pub tool_call_id: String,
}
pub struct Service {
    state: Mutex<ContextState>,
    active: Mutex<Option<Active>>,
    notice: Mutex<Option<Notice>>,
    changes: watch::Sender<u64>,
    operations: Arc<AtomicUsize>,
    closing: AtomicBool,
}
struct Operation(Arc<AtomicUsize>);
impl Drop for Operation {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
impl Service {
    pub fn new(
        config: AppConfig,
        store: Store,
        selected: Option<String>,
        path: Option<PathBuf>,
        state_override: Option<PathBuf>,
    ) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(ContextState {
                epoch: 1,
                config,
                store,
                selected,
                path: path.unwrap_or_else(|| PathBuf::from("wukong.toml")),
                state_override,
            }),
            active: Mutex::new(None),
            notice: Mutex::new(None),
            changes: watch::channel(0).0,
            operations: Arc::new(AtomicUsize::new(0)),
            closing: AtomicBool::new(false),
        })
    }
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }
    pub fn changed(&self) {
        self.changes
            .send_modify(|version| *version = version.wrapping_add(1));
    }
    fn notice(&self, error: bool, message: impl Into<String>) {
        let id = self.changes.borrow().wrapping_add(1);
        *self.notice.lock().unwrap() = Some(Notice {
            id,
            error,
            message: message.into(),
        });
        self.changed();
    }
    fn context(&self, epoch: u64) -> Result<ContextState> {
        let state = self.state.lock().unwrap();
        ensure!(state.epoch == epoch, "工作区配置已变化，请刷新后重试。");
        ensure!(!self.closing.load(Ordering::SeqCst), "工作区正在关闭。");
        Ok(state.clone())
    }
    pub fn status(&self) -> HostStatus {
        let state = self.state.lock().unwrap();
        let active = self.active.lock().unwrap();
        HostStatus {
            epoch: state.epoch,
            initial_run: state.selected.clone(),
            config_path: state.path.clone(),
            state_dir: state.store.root().to_owned(),
            model: state.config.model.model.clone(),
            budget: state.config.budget.clone(),
            execution: active.as_ref().map(|active| active.info.clone()),
            pending_operations: self.operations.load(Ordering::SeqCst),
            notice: self.notice.lock().unwrap().clone(),
        }
    }
    pub fn busy(&self) -> bool {
        self.active.lock().unwrap().is_some() || self.operations.load(Ordering::SeqCst) > 0
    }
    pub fn read(&self, mut request: WorkspaceRequest) -> Result<WorkspaceEnvelope> {
        let context = self.state.lock().unwrap().clone();
        request.run = request.run.or_else(|| context.selected.clone());
        let workspace = workspace::read(&context.store, context.epoch, &request)?;
        let host = self.status();
        ensure!(
            host.epoch == context.epoch,
            "读取期间工作区已切换，请重试。"
        );
        Ok(WorkspaceEnvelope { host, workspace })
    }
    pub async fn start(
        self: &Arc<Self>,
        epoch: u64,
        target: String,
        task: String,
        resume: Option<(String, Budget)>,
    ) -> Result<String> {
        let operation = uuid::Uuid::new_v4().to_string();
        let cancel = CancellationToken::new();
        let context = {
            let state = self.state.lock().unwrap();
            let mut active = self.active.lock().unwrap();
            ensure!(state.epoch == epoch, "工作区已切换，请刷新后重试。");
            ensure!(!self.closing.load(Ordering::SeqCst), "工作区正在关闭。");
            ensure!(
                active.is_none() && self.operations.load(Ordering::SeqCst) == 0,
                "当前操作尚未结束，请等待保存完成。"
            );
            *active = Some(Active {
                info: ExecutionInfo {
                    operation: operation.clone(),
                    run_id: None,
                    phase: "starting",
                },
                cancel: cancel.clone(),
            });
            state.clone()
        };
        self.changed();
        let service = AssessmentService::new(context.config, context.store);
        let prepared = if let Some((run, budget)) = resume {
            service.resume(&run, Some(budget)).await
        } else {
            service
                .start(
                    std::path::Path::new(target.trim().trim_matches('"')),
                    task.trim(),
                )
                .await
        };
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                *self.active.lock().unwrap() = None;
                self.notice(true, format!("启动失败：{error:#}"));
                return Err(error);
            }
        };
        let id = prepared.run().id.clone();
        if let Some(active) = self.active.lock().unwrap().as_mut() {
            active.info.run_id = Some(id.clone());
            active.info.phase = "running";
        }
        self.notice(false, "分析已开始，进展和操作会显示在会话中。");
        let controller = Arc::clone(self);
        let mut events = prepared.subscribe();
        tokio::spawn(async move {
            let mut driver = tokio::spawn(prepared.drive(cancel));
            let outcome = loop {
                tokio::select! {result=&mut driver=>break result.map_err(anyhow::Error::from).and_then(|result|result),event=events.recv()=>{match event{Ok(_)|Err(tokio::sync::broadcast::error::RecvError::Lagged(_))=>controller.changed(),Err(tokio::sync::broadcast::error::RecvError::Closed)=>{break driver.await.map_err(anyhow::Error::from).and_then(|result|result);}}}}
            };
            let mut active = controller.active.lock().unwrap();
            if active
                .as_ref()
                .is_some_and(|active| active.info.operation == operation)
            {
                *active = None;
            }
            drop(active);
            match outcome {
                Ok(run) => controller.notice(
                    run.status == Status::Failed,
                    match run.status {
                        Status::Completed => "分析已完成，可以查看结论与证据。",
                        Status::BudgetPaused => "分析已暂停，进度已保存。",
                        Status::Failed => "分析失败，请查看任务错误与已有证据。",
                        _ => "本轮工作已结束，进度已保存。",
                    },
                ),
                Err(error) => controller.notice(true, format!("分析中断：{error:#}")),
            }
        });
        Ok(id)
    }
    pub fn pause(&self, epoch: u64, run: &str) -> Result<()> {
        let (context, _operation) = self.operation(epoch)?;
        AssessmentService::new(context.config, context.store)
            .pause(run, "operator paused from desktop")?;
        if let Some(active) = self.active.lock().unwrap().as_ref()
            && active.info.run_id.as_deref() == Some(run)
        {
            active.cancel.cancel();
        }
        self.notice(false, "已请求暂停，正在保存进行中的工作。");
        Ok(())
    }
    pub fn stop(&self, epoch: u64, run: &str, agent: &str) -> Result<()> {
        let (context, _operation) = self.operation(epoch)?;
        let record: AgentMetadata = context.store.load(agent)?;
        ensure!(record.run_id == run, "Agent 不属于当前任务");
        AssessmentService::new(context.config, context.store)
            .stop(agent, "operator stopped from desktop")?;
        self.changed();
        Ok(())
    }
    pub fn message(
        &self,
        epoch: u64,
        run: &str,
        agent: &str,
        text: &str,
        follow_up: bool,
        generation: u64,
    ) -> Result<MessageReceipt> {
        let (context, _operation) = self.operation(epoch)?;
        let record: AgentMetadata = context.store.load(agent)?;
        ensure!(record.run_id == run, "Agent 不属于当前任务");
        let receipt = AssessmentService::new(context.config, context.store).message(
            agent,
            text,
            MessageOptions {
                kind: if follow_up {
                    MessageKind::FollowUp
                } else {
                    MessageKind::Information
                },
                recipient_generation: Some(generation),
                ..Default::default()
            },
        )?;
        self.changed();
        Ok(receipt)
    }
    fn operation(&self, epoch: u64) -> Result<(ContextState, Operation)> {
        let state = self.state.lock().unwrap();
        ensure!(state.epoch == epoch, "工作区已切换，请刷新后重试。");
        ensure!(!self.closing.load(Ordering::SeqCst), "工作区正在关闭。");
        self.operations.fetch_add(1, Ordering::SeqCst);
        Ok((state.clone(), Operation(self.operations.clone())))
    }
    pub fn workbench(
        &self,
        epoch: u64,
        request: crate::workbench::DesktopRequest,
    ) -> Result<serde_json::Value> {
        let (context, _operation) = {
            let state = self.state.lock().unwrap();
            let active = self.active.lock().unwrap();
            ensure!(
                state.epoch == epoch && !self.closing.load(Ordering::SeqCst),
                "workspace changed or is closing"
            );
            ensure!(
                active.is_none() && self.operations.load(Ordering::SeqCst) == 0,
                "pause analysis and wait for current operations before changing the environment"
            );
            self.operations.fetch_add(1, Ordering::SeqCst);
            (state.clone(), Operation(self.operations.clone()))
        };
        let workbench = crate::workbench::Workbench::open(&context.config)?;
        let _lock = workbench.lock()?;
        if let crate::workbench::DesktopRequest::Select { id } = &request {
            let config_path = if let Some(id) = id {
                workbench.challenge_dir(id)?.join("wukong.toml")
            } else {
                workbench.root.join("wukong.toml")
            };
            let config = AppConfig::load(Some(&config_path))?;
            let mut state = self.state.lock().unwrap();
            state.config = config;
            state.path = config_path;
            state.epoch += 1;
            drop(state);
            self.changed();
            return Ok(
                serde_json::json!({"selected":id,"new_tasks_use_selected_environment":true,"existing_runs_keep_saved_configuration":true}),
            );
        }
        let result = workbench.desktop(request)?;
        self.changed();
        Ok(result)
    }
    pub fn export(&self, epoch: u64, run: &str) -> Result<Option<PathBuf>> {
        let (context, _operation) = self.operation(epoch)?;
        context.store.require::<RunRecord>("run", run)?;
        let Some(parent) = rfd::FileDialog::new()
            .set_title("选择报告保存目录")
            .pick_folder()
        else {
            return Ok(None);
        };
        let path = parent.join(format!(
            "{run}-{}",
            chrono::Local::now().format("%Y%m%d-%H%M%S")
        ));
        report::export(&context.store, run, &path)?;
        self.notice(false, format!("报告已导出：{}", path.display()));
        Ok(Some(path))
    }
    pub fn configuration(
        &self,
        epoch: u64,
        path: Option<PathBuf>,
    ) -> Result<Option<ConfigDocument>> {
        let (_context, _operation) = self.operation(epoch)?;
        let path = match path {
            Some(path) => path,
            None => match rfd::FileDialog::new()
                .add_filter("TOML", &["toml"])
                .pick_file()
            {
                Some(path) => path,
                None => return Ok(None),
            },
        };
        Ok(Some(ConfigDocument::open(path)?))
    }
    pub fn current_configuration(&self, epoch: u64) -> Result<ConfigDocument> {
        ConfigDocument::open(self.context(epoch)?.path)
    }
    pub fn save_configuration(
        &self,
        epoch: u64,
        mut document: ConfigDocument,
        edits: Vec<ConfigEdit>,
    ) -> Result<ConfigDocument> {
        let mut state = self.state.lock().unwrap();
        let active = self.active.lock().unwrap();
        ensure!(state.epoch == epoch, "工作区已变化，请重新打开设置。");
        ensure!(!self.closing.load(Ordering::SeqCst), "工作区正在关闭。");
        ensure!(
            active.is_none() && self.operations.load(Ordering::SeqCst) == 0,
            "分析或导出尚未结束，请先暂停并等待保存完成。"
        );
        document.patch(&edits)?;
        let (config, store) = document.save(state.state_override.as_deref())?;
        state.config = config;
        state.store = store;
        state.path = document.path.clone();
        state.epoch += 1;
        state.selected = None;
        drop(active);
        drop(state);
        self.notice(
            false,
            "配置已保存。新任务使用新配置，恢复任务沿用历史配置。",
        );
        Ok(document)
    }
    pub fn finding(&self, epoch: u64, run: &str, id: &str) -> Result<FindingDetails> {
        FindingQueries::new(&self.context(epoch)?.store).details(run, id)
    }
    pub fn evidence(&self, epoch: u64, run: &str, id: &str, offset: u64) -> Result<EvidenceView> {
        let context = self.context(epoch)?;
        let preview = FindingQueries::new(&context.store).preview(run, id, offset)?;
        let binary = preview.bytes.contains(&0) || std::str::from_utf8(&preview.bytes).is_err();
        let text = if binary {
            preview
                .bytes
                .chunks(16)
                .enumerate()
                .map(|(i, row)| {
                    format!(
                        "{:08x}  {}  {}",
                        offset + i as u64 * 16,
                        row.iter()
                            .map(|byte| format!("{byte:02x}"))
                            .collect::<Vec<_>>()
                            .join(" "),
                        row.iter()
                            .map(|byte| if byte.is_ascii_graphic() || *byte == b' ' {
                                *byte as char
                            } else {
                                '.'
                            })
                            .collect::<String>()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            String::from_utf8_lossy(&preview.bytes).into_owned()
        };
        Ok(EvidenceView {
            id: preview.evidence.id,
            source: preview.evidence.source,
            media_type: preview.evidence.media_type,
            sha256: preview.evidence.sha256,
            size: preview.evidence.size,
            offset,
            end: offset + preview.bytes.len() as u64,
            text,
            binary,
            agent_id: preview.evidence.agent_id,
            tool_call_id: preview.evidence.tool_call_id,
        })
    }
    pub fn call(&self, epoch: u64, run: &str, id: &str) -> Result<ToolCallRecord> {
        let record: ToolCallRecord = self.context(epoch)?.store.load(id)?;
        ensure!(record.run_id == run, "工具调用不属于当前任务");
        Ok(record)
    }
    pub async fn shutdown(&self) -> Result<()> {
        {
            let _state = self.state.lock().unwrap();
            self.closing.store(true, Ordering::SeqCst);
        }
        if let Some(active) = self.active.lock().unwrap().as_ref() {
            active.cancel.cancel();
        }
        self.changed();
        while self.busy() {
            tokio::time::sleep(std::time::Duration::from_millis(80)).await;
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn invalid_start_releases_execution_and_stale_workspace_commands_fail() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let store = Store::open(directory.path())?;
        let service = Service::new(AppConfig::default(), store.clone(), None, None, None);
        assert!(
            service
                .start(1, "missing".into(), "fixture".into(), None)
                .await
                .is_err()
        );
        assert!(!service.busy());
        assert!(store.runs()?.is_empty());
        assert!(service.pause(0, "run").is_err());
        Ok(())
    }
}
