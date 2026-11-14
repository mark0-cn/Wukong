//! Protected persistent-session intent bound to one admitted image bundle.
use crate::{artifacts::resource_input::Reference, executors::resource_process};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

pub(crate) fn prefix(kind: crate::resources::models::ResourceKind) -> Result<&'static str> {
    match kind {
        crate::resources::models::ResourceKind::Debugger => Ok("debug"),
        crate::resources::models::ResourceKind::Interactive => Ok("interact"),
        _ => anyhow::bail!("native resource is not a persistent session"),
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Request {
    pub schema: u32,
    pub target: resource_process::Request,
    pub mode: Mode,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Mode {
    Launch,
    Dump { file: Reference },
    Interactive,
}
impl Request {
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == crate::executors::session_host::SCHEMA,
            "unsupported native session request schema"
        );
        self.target.validate()?;
        ensure!(
            self.target.interpreter.is_none() && self.target.replay.is_none(),
            "native session admission requires a native target image"
        );
        if self.kind() == crate::resources::models::ResourceKind::Interactive {
            ensure!(
                self.target.input.is_none()
                    || self
                        .target
                        .launch
                        .as_ref()
                        .is_some_and(|launch| launch.input_mode
                            == crate::executors::resource_launch::InputMode::ArgvFile),
                "interactive stdin is supplied through serialized writes; initial input requires argv_file"
            );
        }
        if let Some(dump) = self.dump() {
            ensure!(
                self.target.input.is_none(),
                "dump inspection cannot supply stdin"
            );
            ensure!(
                !dump.executable && dump.size <= crate::executors::session_identity::MAX_DUMP,
                "dump must be non-executable and within its inspection bound"
            );
            let mut files = if let Some(launch) = &self.target.launch {
                launch.image_files(&self.target.program, None)?
            } else {
                self.target.argument_files.clone()
            };
            if let Some(member) = files.iter().find(|member| member.path == dump.path) {
                ensure!(
                    member == dump,
                    "dump aliases a different admitted bundle member"
                );
            } else {
                files.push(dump.clone());
            }
            crate::artifacts::resource_image::validate_paths(&self.target.program, &files)?;
        }
        ensure!(
            serde_json::to_vec(self)?.len() <= 65536,
            "debugger request exceeds protected record bound"
        );
        Ok(())
    }
    pub(crate) fn dump(&self) -> Option<&Reference> {
        match &self.mode {
            Mode::Launch | Mode::Interactive => None,
            Mode::Dump { file } => Some(file),
        }
    }
    pub(crate) fn read_only(&self) -> bool {
        self.dump().is_some()
    }
    pub(crate) fn kind(&self) -> crate::resources::models::ResourceKind {
        match self.mode {
            Mode::Interactive => crate::resources::models::ResourceKind::Interactive,
            _ => crate::resources::models::ResourceKind::Debugger,
        }
    }
    pub(crate) fn scope(&self) -> &'static str {
        match self.mode {
            Mode::Interactive => "native_interactive_host",
            _ => "native_debugger_host",
        }
    }
}
