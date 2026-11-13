//! Protected debugger intent. The process description binds an image bundle;
//! only Launch authorizes creating its target process.
use crate::{resource_input::Reference, resource_process};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

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
}
impl Request {
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == crate::session_host::SCHEMA,
            "unsupported debugger request schema"
        );
        self.target.validate()?;
        ensure!(
            self.target.interpreter.is_none(),
            "debugger admission requires a native target image"
        );
        if let Some(dump) = self.dump() {
            ensure!(
                self.target.input.is_none(),
                "dump inspection cannot supply stdin"
            );
            ensure!(
                !dump.executable && dump.size <= crate::session_identity::MAX_DUMP,
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
            crate::resource_image::validate_paths(&self.target.program, &files)?;
        }
        ensure!(
            serde_json::to_vec(self)?.len() <= 65536,
            "debugger request exceeds protected record bound"
        );
        Ok(())
    }
    pub(crate) fn dump(&self) -> Option<&Reference> {
        match &self.mode {
            Mode::Launch => None,
            Mode::Dump { file } => Some(file),
        }
    }
    pub(crate) fn read_only(&self) -> bool {
        self.dump().is_some()
    }
}
