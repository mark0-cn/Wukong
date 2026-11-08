//! State is scoped by its lifetime: projection, selection, composer, reading and dialogs.
use super::*;
#[derive(Default)]
pub(super) struct WorkspaceData {
    pub runs: Vec<RunSummary>,
    pub agents: Vec<AgentSummary>,
    pub events: Vec<Event>,
    pub calls: Vec<ToolCallRecord>,
    pub notes: Vec<SharedDocument>,
    pub findings: Vec<Finding>,
    pub operator_messages: Vec<Message>,
    pub conversation: Option<crate::gui_chat::Document>,
}
pub(super) struct Selection {
    pub selected: Option<String>,
    pub agent: Option<String>,
    pub view: View,
    pub filter: String,
    pub event_limit: usize,
    pub selection_epoch: u64,
    pub request_serial: u64,
}
impl Default for Selection {
    fn default() -> Self {
        Self {
            selected: None,
            agent: None,
            view: View::Activity,
            filter: String::new(),
            event_limit: 500,
            selection_epoch: 0,
            request_serial: 0,
        }
    }
}
#[derive(Default)]
pub(super) struct Composer {
    pub message: String,
    pub follow_up: bool,
}
pub(super) struct ChatView {
    pub chat_follow: bool,
    pub chat_jump: bool,
    pub chat_details: bool,
    pub chat_layout: crate::gui_chat::Layout,
}
impl Default for ChatView {
    fn default() -> Self {
        Self {
            chat_follow: true,
            chat_jump: true,
            chat_details: false,
            chat_layout: Default::default(),
        }
    }
}
pub(super) struct TaskDraft {
    pub target: String,
    pub task: String,
}
impl Default for TaskDraft {
    fn default() -> Self {
        Self {
            target: String::new(),
            task: DEFAULT_TASK.into(),
        }
    }
}
#[derive(Default)]
pub(super) enum TaskDialog {
    #[default]
    None,
    New,
    Resume(Box<RunSummary>),
}
#[derive(Default, PartialEq, Eq)]
pub(super) enum CloseState {
    #[default]
    Open,
    Confirming,
    Waiting,
    Allowed,
}
#[derive(Default)]
pub(super) struct Dialogs {
    pub task: TaskDialog,
    pub stop: Option<StopTarget>,
    pub close: CloseState,
}
pub(super) enum Execution {
    Idle,
    Starting(CancellationToken),
    Running {
        id: String,
        cancel: CancellationToken,
    },
}
impl Execution {
    pub fn busy(&self) -> bool {
        !matches!(self, Self::Idle)
    }
    pub fn cancel(&self) -> Option<&CancellationToken> {
        match self {
            Self::Idle => None,
            Self::Starting(cancel) | Self::Running { cancel, .. } => Some(cancel),
        }
    }
    pub fn run(&self) -> Option<&String> {
        match self {
            Self::Running { id, .. } => Some(id),
            _ => None,
        }
    }
    pub fn started(&mut self, id: String) {
        if let Some(cancel) = self.cancel().cloned() {
            *self = Self::Running { id, cancel };
        }
    }
}
