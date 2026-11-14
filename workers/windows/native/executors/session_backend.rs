//! Shared contained WinDbg MCP lifecycle for sessions and crash observations.
use crate::{
    executors::debugger_mcp::Client,
    executors::resource_process::ScratchIntent,
    executors::session_request::Request,
    executors::session_target::Prepared,
    host::accounts::Role,
    host::resource_host::Host,
    isolation::persona::Persona,
    isolation::process_capture::{CaptureResult, CapturedStream},
    isolation::processes::{OwnedProcess, ResourceLimits, Spawn},
    isolation::security::{self, AclPolicy},
};
use anyhow::{Context, Result};
use serde_json::Value;
use std::{
    fs::File,
    io::{PipeReader, PipeWriter},
    os::windows::io::OwnedHandle,
    path::{Path, PathBuf},
    time::Duration,
};

pub(crate) struct Backend {
    pub target: Prepared,
    pub helper: OwnedProcess,
    pub diagnostics: CapturedStream,
    debugger: Persona,
    directory: PathBuf,
    transport: Option<(PipeReader, PipeWriter, File)>,
}
pub(crate) struct Closed {
    pub captured: Option<CaptureResult>,
    pub diagnostics: Vec<u8>,
    pub identity: Value,
}
impl Backend {
    pub(crate) fn launch(
        host: &Host,
        request: &Request,
        nonce: &str,
        transcript: File,
        prepare: impl FnOnce(&Persona, &Path) -> Result<Prepared>,
    ) -> Result<Self> {
        let root = &host.root;
        let private = AclPolicy::private_vm();
        let debugger = host.persona(Role::Debugger, nonce, &request.target.resource_id)?;
        crate::artifacts::io::publish_json(
            &root
                .join("metadata")
                .join(format!("resource-scratch-{nonce}.json")),
            &ScratchIntent {
                schema: crate::executors::resource_process::SCHEMA,
                installation_id: host.installation.clone(),
                resource_id: request.target.resource_id.clone(),
                appcontainer_sid: debugger.sid_text()?,
                logon_sid: debugger.logon_sid().into(),
            },
            &private,
        )?;
        let directory = security::ResourceDirectory::create(
            &host.resources.join(nonce),
            &debugger.sid_text()?,
            debugger.logon_sid(),
        )?;
        let path = directory.path().to_owned();
        let mut target = prepare(&debugger, &path)?;
        let executable = host.package.join(
            host.manifest
                .manifest()
                .entrypoints
                .windbg_mcp
                .as_ref()
                .context("package has no WinDbg MCP backend")?,
        );
        let (input, writer) = std::io::pipe()?;
        let (reader, output) = std::io::pipe()?;
        let input = File::from(OwnedHandle::from(input));
        let output = File::from(OwnedHandle::from(output));
        let (diagnostic_reader, diagnostic_writer) = std::io::pipe()?;
        let log = File::from(OwnedHandle::from(diagnostic_writer));
        let mut environment =
            crate::host::resource_host::scratch_environment(&host.environment, &path);
        for key in [
            "_NT_SYMBOL_PATH",
            "_NT_ALT_SYMBOL_PATH",
            "_NT_EXECUTABLE_IMAGE_PATH",
        ] {
            environment.insert(key.into(), target.symbols().as_os_str().to_owned());
        }
        let helper = Spawn {
            executable: &executable,
            arguments: &[
                "--tools".into(),
                if request.read_only() {
                    "session,inspect,crash"
                } else {
                    "session,inspect,exec,crash"
                }
                .into(),
            ],
            directory: &path,
            environment: &environment,
            log: &output,
            input: &input,
        }
        .sandbox_streamed(
            &debugger,
            directory,
            &log,
            ResourceLimits::new(4, 1024 * 1024 * 1024)?,
        )?;
        drop((input, output));
        target.launch(request, &debugger)?;
        drop(log);
        Ok(Self {
            target,
            helper,
            debugger,
            directory: path,
            diagnostics: CapturedStream::start(diagnostic_reader, 1024 * 1024)?,
            transport: Some((reader, writer, transcript)),
        })
    }
    pub(crate) async fn connect(&mut self) -> Result<Client> {
        let (reader, writer, transcript) = self
            .transport
            .take()
            .context("MCP backend already connected")?;
        Client::connect(
            crate::rpc::stream_io::reader(reader),
            crate::rpc::stream_io::writer(writer),
            transcript,
        )
        .await
    }
    pub(crate) fn close(mut self) -> Result<Closed> {
        // Drain both Jobs even if either stop fails. Recovery owns uncertain cleanup.
        let helper = self.helper.stop(Duration::from_secs(10));
        let target = self.target.stop();
        drop(self.helper);
        helper?;
        let captured = target?;
        let diagnostics = self.diagnostics.finish()?;
        let identity = serde_json::to_value(self.target.identity()?)?;
        self.target.close()?;
        self.debugger.close()?;
        AclPolicy::private_vm().verify(&self.directory, true)?;
        crate::isolation::resource_cleanup::remove_reclaimed(&self.directory)?;
        Ok(Closed {
            captured,
            diagnostics,
            identity,
        })
    }
}
