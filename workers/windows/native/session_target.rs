//! One session's live process or immutable dump, behind the same MCP lifecycle.
//! Dump preparation never creates a target persona, process or output stream.
use crate::{
    debugger_mcp::Client,
    persona::Persona,
    process_capture::{CaptureResult, CaptureWatch, CapturedProcess, PreparedIo},
    processes::ResourceLimits,
    resource_host::Host,
    resource_input::{self, Purpose},
    resource_target,
    security::AclPolicy,
    session_identity::Identity,
    session_request::Request,
};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

pub(crate) enum Prepared {
    Launch {
        image: Box<resource_target::Prepared>,
        process: Option<CapturedProcess>,
    },
    Dump {
        identity: Identity,
        path: PathBuf,
        symbols: PathBuf,
        held: Vec<File>,
    },
}
impl Prepared {
    pub(crate) fn prepare(
        host: &Host,
        request: &Request,
        debugger: &Persona,
        scratch: &Path,
    ) -> Result<Self> {
        let Some(dump) = request.dump() else {
            return Ok(Self::Launch {
                image: Box::new(resource_target::Prepared::open(
                    host,
                    &request.target,
                    Some(debugger),
                )?),
                process: None,
            });
        };
        let mut sources = resource_target::image_sources(host, &request.target)?;
        let mut identity = Identity::admit(&mut sources)?;
        if let Some(input) = sources.iter_mut().find(|input| input.reference() == dump) {
            identity.admit_dump(input)?;
        } else {
            let mut input = resource_input::open(
                &host.root.join("metadata"),
                &request.target.owner,
                dump,
                Purpose::Inspect,
            )?;
            identity.admit_dump(&mut input)?;
            sources.push(input);
        }
        let policy = AclPolicy::private_vm()
            .with_resource_code(&debugger.sid_text()?, debugger.logon_sid())?;
        let held = crate::resource_image::stage(scratch, sources, &policy, None, false)?;
        let symbols = scratch
            .join(&request.target.program.path)
            .parent()
            .context("dump image parent missing")?
            .to_owned();
        Ok(Self::Dump {
            identity,
            path: scratch.join(&dump.path),
            symbols,
            held,
        })
    }
    pub(crate) fn launch(&mut self, request: &Request, debugger: &Persona) -> Result<()> {
        if let Self::Launch { image, process } = self {
            let input = image
                .input
                .take()
                .map(|input| Box::new(input) as Box<dyn Read + Send>);
            *process = Some(
                PreparedIo::new(request.target.output_limit)?.launch(input, |io| {
                    image.launch_with_debugger(
                        io,
                        ResourceLimits::new(request.target.processes, request.target.memory_bytes)?,
                        Some(debugger),
                    )
                })?,
            );
        }
        Ok(())
    }
    pub(crate) async fn open(&mut self, client: &mut Client) -> Result<Value> {
        match self {
            Self::Launch { process, .. } => {
                client
                    .attach_owned(
                        process
                            .as_mut()
                            .context("debugger target not launched")?
                            .process_mut(),
                    )
                    .await
            }
            Self::Dump { path, .. } => client.structured("open_dump", json!({"path":path})).await,
        }
    }
    pub(crate) fn identity(&self) -> Result<&Identity> {
        match self {
            Self::Launch { image, .. } => image
                .identity
                .as_ref()
                .context("admitted debugger identity missing"),
            Self::Dump { identity, .. } => Ok(identity),
        }
    }
    pub(crate) fn profile(&mut self) -> Value {
        let observed = match self {
            Self::Launch { process, .. } => process
                .as_mut()
                .context("debugger target not launched")
                .and_then(|process| process.process_mut().profile()),
            Self::Dump { .. } => {
                return json!({"status":"unavailable","reason":"dump has no live owned process"});
            }
        };
        observed.unwrap_or_else(|error| {
            json!({"status":"unavailable",
            "error":format!("{error:#}").chars().take(512).collect::<String>()})
        })
    }
    pub(crate) fn symbols(&self) -> &Path {
        match self {
            Self::Launch { image, .. } => image.symbols(),
            Self::Dump { symbols, .. } => symbols,
        }
    }
    pub(crate) fn watch(&self) -> Option<CaptureWatch> {
        match self {
            Self::Launch { process, .. } => process.as_ref().map(CapturedProcess::watch),
            Self::Dump { .. } => None,
        }
    }
    pub(crate) fn stop(&mut self) -> Result<Option<CaptureResult>> {
        match self {
            Self::Launch { process, .. } => process
                .take()
                .map(|p| p.wait_with_cancel(Duration::from_secs(10), || Ok(true)))
                .transpose(),
            Self::Dump { .. } => Ok(None),
        }
    }
    /// Called after both the upstream debugger and any owned target have drained.
    pub(crate) fn close(self) -> Result<()> {
        match self {
            Self::Launch { image, .. } => image.close(),
            Self::Dump { held, .. } => {
                drop(held);
                Ok(())
            }
        }
    }
}
