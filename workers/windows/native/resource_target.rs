//! Admitted target preparation and cleanup, independent of how the target is
//! driven. The System host retains source/image handles and its exact persona.
use crate::{
    accounts::Role,
    deployment,
    persona::Persona,
    process_capture::PreparedIo,
    processes::{OwnedProcess, ResourceLimits, Spawn},
    resource_host::Host,
    resource_input::{self, AdmittedInput, Purpose},
    resource_process::{Request, SCHEMA, ScratchIntent, nonce},
    security::{self, AclPolicy, ResourceDirectory},
};
use anyhow::{Context, Result};
use std::{collections::BTreeMap, ffi::OsString, fs::File, path::PathBuf};

pub(crate) struct Prepared {
    directory: Option<ResourceDirectory>,
    working: Option<ResourceDirectory>,
    held: Vec<File>,
    pub input: Option<AdmittedInput>,
    pub persona: Persona,
    scratch: PathBuf,
    executable: PathBuf,
    cwd: PathBuf,
    arguments: Vec<OsString>,
    environment: BTreeMap<String, OsString>,
}
impl Prepared {
    /// The caller has already checked the protected request and guardian Job.
    /// Resolve all inputs before creating the target's persona or scratch.
    pub(crate) fn open(host: &Host, request: &Request) -> Result<Self> {
        let metadata = host.root.join("metadata");
        if let Some(launch) = &request.launch {
            let document = crate::resource_launch::Document::read(resource_input::open(
                &metadata,
                &request.owner,
                &launch.manifest,
                Purpose::Inspect,
            )?)?;
            launch.verify(
                &request.program,
                request.input.as_ref(),
                document,
                &request.arguments,
                &request.environment,
                request.timeout_secs,
            )?;
        }
        let image_files = if let Some(launch) = &request.launch {
            launch.image_files(&request.program, request.input.as_ref())?
        } else {
            request.argument_files.clone()
        };
        let sources = crate::resource_image::acquire(
            &metadata,
            &request.owner,
            &request.program,
            &image_files,
        )?;
        let input = request
            .input
            .as_ref()
            .filter(|_| {
                request.launch.as_ref().is_none_or(|launch| {
                    launch.input_mode == crate::resource_launch::InputMode::StdinPipe
                })
            })
            .map(|input| resource_input::open(&metadata, &request.owner, input, Purpose::Inspect))
            .transpose()?;
        let installation = &host.deployment.installation_id;
        let nonce = nonce(installation, &request.resource_id)?;
        let (persona, _) = crate::resource_host::create_persona_named(
            &host.deployment,
            &host.root,
            Role::Target,
            &nonce,
            Some(&request.resource_id),
        )?;
        let private = AclPolicy::private_vm();
        let image_policy = private
            .clone()
            .with_resource_code(&persona.sid_text()?, persona.logon_sid())?;
        deployment::publish_json(
            &metadata.join(format!("resource-scratch-{nonce}.json")),
            &ScratchIntent {
                schema: SCHEMA,
                installation_id: installation.clone(),
                resource_id: request.resource_id.clone(),
                appcontainer_sid: persona.sid_text()?,
                logon_sid: persona.logon_sid().into(),
            },
            &private,
        )?;
        let directory = ResourceDirectory::create(
            &host.resources.join(&nonce),
            &persona.sid_text()?,
            persona.logon_sid(),
        )?;
        let scratch = directory.path().to_owned();
        // acquire() checked each portable files/... member. Preserve its layout.
        let executable = scratch.join(&request.program.path);
        let working = request
            .launch
            .as_ref()
            .map(|launch| {
                crate::resource_image::working_directory(
                    &scratch,
                    &launch.cwd,
                    &persona.sid_text()?,
                    persona.logon_sid(),
                    &image_policy,
                )
            })
            .transpose()?;
        let cwd = working
            .as_ref()
            .map(|directory| directory.path())
            .unwrap_or(&scratch)
            .to_owned();
        let held =
            crate::resource_image::stage(&scratch, sources, &image_policy, working.as_ref())?;
        let mut environment =
            crate::resource_host::scratch_environment(&host.environment, &scratch);
        environment.extend(
            request
                .environment
                .iter()
                .map(|(key, value)| (key.clone(), OsString::from(value))),
        );
        let arguments = request
            .arguments
            .iter()
            .map(|argument| {
                if argument == "@@"
                    && request.launch.as_ref().is_some_and(|launch| {
                        launch.input_mode == crate::resource_launch::InputMode::ArgvFile
                    })
                {
                    Ok(scratch
                        .join(
                            &request
                                .input
                                .as_ref()
                                .context("argv_file input disappeared")?
                                .path,
                        )
                        .into_os_string())
                } else {
                    Ok(OsString::from(argument))
                }
            })
            .collect::<Result<_>>()?;
        Ok(Self {
            persona,
            input,
            directory: Some(directory),
            working,
            held,
            scratch,
            executable,
            cwd,
            arguments,
            environment,
        })
    }

    pub(crate) fn launch(
        &mut self,
        io: &PreparedIo,
        limits: ResourceLimits,
    ) -> Result<OwnedProcess> {
        let directory = self.directory.take().context("target already launched")?;
        Spawn {
            executable: &self.executable,
            arguments: &self.arguments,
            directory: &self.cwd,
            environment: &self.environment,
            log: io.output(),
            input: io.input(),
        }
        .sandbox_streamed(&self.persona, directory, io.error(), limits)
    }

    /// Only after the owned process Job has drained (or before launch).
    /// Uncertain failures retain the published intents for resource recovery.
    pub(crate) fn close(mut self) -> Result<()> {
        drop(self.input.take());
        self.held.clear();
        if let Some(working) = self.working.take() {
            working.reclaim()?;
            drop(working);
        }
        if let Some(directory) = self.directory.take() {
            directory.reclaim()?;
            drop(directory);
        }
        self.persona.close()?;
        AclPolicy::private_vm().verify(&self.scratch, true)?;
        security::no_links(&self.scratch)?;
        crate::resource_cleanup::remove_reclaimed(&self.scratch)
    }
}
