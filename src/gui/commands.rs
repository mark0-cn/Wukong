//! Blocking application commands and native dialogs execute away from the render thread.
use super::*;
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Key {
    Pause(String),
    Stop(String),
    Message(String),
    Export,
    Target,
    Settings,
    Finding,
    Evidence,
}
pub(super) enum Command {
    Finding {
        run: String,
        id: String,
        revision: i64,
    },
    Evidence {
        run: String,
        id: String,
        offset: u64,
    },
    Pause(String),
    Stop(String),
    Message {
        recipient: String,
        text: String,
        options: MessageOptions,
    },
    Export(String),
    Target,
    OpenSettings(Option<PathBuf>),
    SaveSettings {
        editor: Box<crate::gui_settings::ConfigEditor>,
        state_override: Option<PathBuf>,
    },
}
pub(super) enum Outcome {
    Finding {
        id: String,
        revision: i64,
        details: Box<crate::queries::FindingDetails>,
    },
    Evidence(Box<crate::queries::EvidencePreview>),
    Paused(String),
    Stopped,
    Sent {
        recipient: String,
        text: String,
    },
    Exported(Option<PathBuf>),
    Target(Option<PathBuf>),
    SettingsOpened(Option<Box<crate::gui_settings::ConfigEditor>>),
    SettingsSaved {
        editor: Box<crate::gui_settings::ConfigEditor>,
        config: Box<AppConfig>,
        store: Store,
    },
}
impl Command {
    fn key(&self) -> Key {
        match self {
            Self::Finding { .. } => Key::Finding,
            Self::Evidence { .. } => Key::Evidence,
            Self::Pause(id) => Key::Pause(id.clone()),
            Self::Stop(id) => Key::Stop(id.clone()),
            Self::Message { recipient, .. } => Key::Message(recipient.clone()),
            Self::Export(_) => Key::Export,
            Self::Target => Key::Target,
            Self::OpenSettings(_) | Self::SaveSettings { .. } => Key::Settings,
        }
    }
    fn execute(self, config: AppConfig, store: Store) -> Result<Outcome> {
        let service = AssessmentService::new(config, store.clone());
        Ok(match self {
            Self::Finding { run, id, revision } => Outcome::Finding {
                details: Box::new(crate::queries::FindingQueries::new(&store).details(&run, &id)?),
                id,
                revision,
            },
            Self::Evidence { run, id, offset } => Outcome::Evidence(Box::new(
                crate::queries::FindingQueries::new(&store).preview(&run, &id, offset)?,
            )),
            Self::Pause(id) => {
                service.pause(&id, "operator paused from desktop")?;
                Outcome::Paused(id)
            }
            Self::Stop(id) => {
                service.stop(&id, "operator stopped from desktop")?;
                Outcome::Stopped
            }
            Self::Message {
                recipient,
                text,
                options,
            } => {
                service.message(&recipient, &text, options)?;
                Outcome::Sent { recipient, text }
            }
            Self::Export(id) => {
                let path = rfd::FileDialog::new()
                    .set_title("选择报告保存目录")
                    .pick_folder()
                    .map(|parent| {
                        parent.join(format!(
                            "{id}-{}",
                            chrono::Local::now().format("%Y%m%d-%H%M%S")
                        ))
                    });
                if let Some(path) = &path {
                    report::export(&store, &id, path)?;
                }
                Outcome::Exported(path)
            }
            Self::Target => Outcome::Target(rfd::FileDialog::new().pick_file()),
            Self::OpenSettings(path) => {
                let path = path.or_else(|| {
                    rfd::FileDialog::new()
                        .add_filter("TOML", &["toml"])
                        .pick_file()
                });
                Outcome::SettingsOpened(
                    path.map(crate::gui_settings::ConfigEditor::open)
                        .transpose()?
                        .map(Box::new),
                )
            }
            Self::SaveSettings {
                mut editor,
                state_override,
            } => {
                let (config, store) = editor.save(state_override.as_deref())?;
                Outcome::SettingsSaved {
                    editor,
                    config: Box::new(config),
                    store,
                }
            }
        })
    }
}
pub(super) struct Controller {
    pub context: Option<egui::Context>,
    pending: BTreeSet<Key>,
    runtime: tokio::runtime::Handle,
    tx: mpsc::Sender<Update>,
}
impl Controller {
    pub fn new(runtime: tokio::runtime::Handle, tx: mpsc::Sender<Update>) -> Self {
        Self {
            context: None,
            pending: BTreeSet::new(),
            runtime,
            tx,
        }
    }
    pub fn busy(&self) -> bool {
        !self.pending.is_empty()
    }
    pub fn contains(&self, key: &Key) -> bool {
        self.pending.contains(key)
    }
    pub fn complete(&mut self, key: &Key) {
        self.pending.remove(key);
    }
    pub fn submit(&mut self, command: Command, config: AppConfig, store: Store) {
        let key = command.key();
        if !self.pending.insert(key.clone()) {
            return;
        }
        let root = store.root().to_owned();
        let tx = self.tx.clone();
        let context = self.context.clone();
        let work = self
            .runtime
            .spawn_blocking(move || command.execute(config, store));
        self.runtime.spawn(async move {
            let result = work
                .await
                .map_err(anyhow::Error::from)
                .and_then(|result| result);
            let _ = tx.send(Update::CommandFinished { root, key, result });
            if let Some(context) = context {
                context.request_repaint();
            }
        });
    }
}
