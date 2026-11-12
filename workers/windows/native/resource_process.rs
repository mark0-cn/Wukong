//! One System child per native execution resource. Controller admission owns
//! the fence and journal; this host accepts only its protected fixed request.
use crate::{
    accounts::Role,
    deployment,
    inputs::digest,
    process_capture::{CaptureResult, PreparedIo},
    processes::{ResourceLimits, Spawn},
    resource_input::{self, Purpose, Reference},
    resource_journal::{Binding, Owner},
    security::{self, AclPolicy},
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    io::{Read, Write},
    os::windows::fs::OpenOptionsExt,
    path::Path,
    time::Duration,
};
use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub schema: u32,
    pub resource_id: String,
    pub owner: Owner,
    pub program: Reference,
    pub arguments: Vec<String>,
    pub input: Option<Reference>,
    pub environment: BTreeMap<String, String>,
    pub timeout_secs: u64,
    pub output_limit: usize,
    pub processes: u32,
    pub memory_bytes: u64,
}
impl Request {
    pub fn validate(&self) -> Result<()> {
        crate::resource_journal::bounded_identifier(&self.owner.run_id, 128)?;
        crate::resource_journal::bounded_identifier(&self.owner.owner_id, 128)?;
        ensure!(
            self.schema == 1
                && self.resource_id.len() == 32
                && self
                    .resource_id
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
            "invalid native process request identity"
        );
        ensure!(
            self.owner.epoch > 0
                && !self.owner.run_id.is_empty()
                && !self.owner.owner_id.is_empty(),
            "missing admitted native process owner"
        );
        Binding::from_readiness(
            serde_json::to_value(&self.owner.identity)?,
            &self.owner.identity.installation_id,
            &self.owner.identity.package_sha256,
        )?;
        ensure!(
            self.program.executable
                && self.arguments.len() <= 128
                && self.arguments.iter().all(|a| !a.contains('\0'))
                && self
                    .arguments
                    .iter()
                    .map(|a| a.encode_utf16().count())
                    .sum::<usize>()
                    <= 16384,
            "invalid native process argv or executable policy"
        );
        ensure!(
            (1..=86400).contains(&self.timeout_secs)
                && (1024..=8 * 1024 * 1024).contains(&self.output_limit),
            "invalid native process deadline or output bound"
        );
        ensure!(
            self.input
                .as_ref()
                .is_none_or(|input| input.size <= 32 * 1024 * 1024),
            "native process stdin exceeds 32 MiB"
        );
        ensure!(
            self.environment.len() <= 64,
            "too many native process environment overrides"
        );
        for (key, value) in &self.environment {
            let approved = matches!(key.as_str(), "ASAN_OPTIONS" | "UBSAN_OPTIONS")
                || key
                    .strip_prefix("AFL_")
                    .or_else(|| key.strip_prefix("QEMU_"))
                    .is_some_and(|name| {
                        !name.is_empty()
                            && name
                                .bytes()
                                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
                    });
            ensure!(
                approved && value.len() <= 4096 && !value.contains('\0'),
                "native process environment override is not permitted"
            );
        }
        ResourceLimits::new(self.processes, self.memory_bytes)?;
        Ok(())
    }
}

/// This is an internal System host entry, not a metadata-worker execution API.
/// The native service must validate the current controller lease, reserve its
/// durable resource intent and publish this exact protected request first.
pub fn execute(
    installation: &str,
    configuration: &Path,
    request_path: &Path,
) -> Result<serde_json::Value> {
    security::require_system()?;
    let crate::resource_host::Host {
        deployment,
        root,
        manifest,
        resources,
        environment: system_environment,
    } = crate::resource_host::open(installation, configuration)?;
    let private = AclPolicy::private_vm();
    private.verify(request_path, false)?;
    let request: Request = serde_json::from_slice(&deployment::read_bytes(request_path, 65536)?)?;
    request.validate()?;
    let expected = root
        .join("metadata/resource-requests")
        .join(format!("{}.json", request.resource_id));
    ensure!(
        expected.canonicalize()? == request_path.canonicalize()?,
        "native process request is outside its fixed protected slot"
    );
    ensure!(
        request.owner.identity.installation_id == installation
            && request.owner.identity.package_sha256 == manifest.sha256(),
        "native process request belongs to another signed deployment"
    );
    let public = deployment.code_policy()?;
    let metadata = root.join("metadata");
    // Reject incomplete/mismatched inputs before creating a persona or scratch.
    let program = resource_input::open(
        &metadata,
        &request.owner,
        &request.program,
        Purpose::Execute,
    )?;
    let input = request
        .input
        .as_ref()
        .map(|input| resource_input::open(&metadata, &request.owner, input, Purpose::Inspect))
        .transpose()?;
    let nonce = digest(&(
        "native.process.persona.v1",
        installation,
        &request.resource_id,
    ))?[..48]
        .to_owned();
    let (mut persona, _) = crate::resource_host::create_persona_named(
        &deployment,
        &root,
        Role::Target,
        &nonce,
        Some(&request.resource_id),
    )?;
    let directory = security::ResourceDirectory::create(
        &resources.join(&nonce),
        &persona.sid_text()?,
        persona.logon_sid(),
    )?;
    let scratch = directory.path().to_owned();
    // Keep controller logical paths valid in the private working directory.
    // acquire() already checked this portable relative files/... spelling.
    let executable = scratch.join(&request.program.path);
    let relative = Path::new(&request.program.path);
    let mut parents: Vec<_> = relative
        .parent()
        .into_iter()
        .flat_map(Path::ancestors)
        .filter(|parent| !parent.as_os_str().is_empty())
        .collect();
    parents.reverse();
    for parent in parents {
        let path = scratch.join(parent);
        if !path.exists() {
            public.create_directory(&path)?;
        }
        public.verify(&path, true)?;
    }
    let held = stage_program(program, &executable, &request.program, public)?;
    let mut child_environment =
        crate::resource_host::scratch_environment(&system_environment, &scratch);
    child_environment.extend(
        request
            .environment
            .iter()
            .map(|(key, value)| (key.clone(), OsString::from(value))),
    );
    let arguments: Vec<_> = request.arguments.iter().map(OsString::from).collect();
    let limits = ResourceLimits::new(request.processes, request.memory_bytes)?;
    let process = PreparedIo::new(request.output_limit)?.launch(
        input.map(|input| Box::new(input) as Box<dyn Read + Send>),
        |io| {
            Spawn {
                executable: &executable,
                arguments: &arguments,
                directory: &scratch,
                environment: &child_environment,
                log: io.output(),
                input: io.input(),
            }
            .sandbox_streamed(&persona, directory, io.error(), limits)
        },
    )?;
    let appcontainer = persona.sid_text()?;
    let logon = persona.logon_sid().to_owned();
    let cancel = root
        .join("metadata/resource-requests")
        .join(format!("{}.cancel", request.resource_id));
    let result = process.wait_with_cancel(Duration::from_secs(request.timeout_secs), || {
        Ok(cancel.try_exists()?)
    })?;
    drop(held);
    persona.close()?;
    private.verify(&scratch, true)?;
    security::no_links(&scratch)?;
    // Quiescence and held-root reclaim precede this call. Rust's Windows
    // remover opens descendants relative to held directories without following
    // junctions. A restrictive descendant still makes cleanup fail closed.
    fs::remove_dir_all(&scratch)?;
    finish(&request, result, appcontainer, logon)
}

/// Publish full binary-output evidence only after successful owned cleanup.
/// The service reads this protected slot; target output never reaches its log.
pub fn run(installation: &str, configuration: &Path, request: &Path) -> Result<serde_json::Value> {
    let value = execute(installation, configuration, request)?;
    let (_, root) = deployment::load(configuration)?;
    let directory = root.join("metadata/resource-results");
    let policy = AclPolicy::private_vm();
    policy.verify(&directory, true)?;
    let id = value["resource_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("native host result has no identity"))?;
    deployment::write_new_json(&directory.join(format!("{id}.json")), &value, &policy)?;
    Ok(serde_json::json!({"resource_id":id,"settled":true}))
}

fn stage_program(
    mut source: resource_input::AdmittedInput,
    destination: &Path,
    reference: &Reference,
    policy: AclPolicy,
) -> Result<fs::File> {
    let private = AclPolicy::private_vm();
    let mut file = private.create_file(destination)?;
    ensure!(
        std::io::copy(&mut source, &mut file)? == reference.size,
        "native staged executable length differs"
    );
    file.flush()?;
    file.sync_all()?;
    drop(file);
    policy.apply(destination, false)?;
    Ok(fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(destination)?)
}
fn finish(
    request: &Request,
    result: CaptureResult,
    appcontainer: String,
    logon: String,
) -> Result<serde_json::Value> {
    use base64::Engine;
    use sha2::{Digest, Sha256};
    let reason = match result.reason {
        crate::process_capture::StopReason::Exited => "exited",
        crate::process_capture::StopReason::Deadline => "deadline",
        crate::process_capture::StopReason::OutputLimit => "output_limit",
        crate::process_capture::StopReason::IoFailure => "io_failure",
        crate::process_capture::StopReason::Cancelled => "cancelled",
    };
    Ok(
        serde_json::json!({"schema":1,"resource_id":request.resource_id,"owner":request.owner,"program":request.program,
        "pid":result.pid,"exit_code":result.exit_code,"stop_reason":reason,"appcontainer_sid":appcontainer,"logon_sid":logon,
        "stdout_base64":base64::engine::general_purpose::STANDARD.encode(&result.stdout),"stderr_base64":base64::engine::general_purpose::STANDARD.encode(&result.stderr),
        "stdout_sha256":hex::encode(Sha256::digest(&result.stdout)),"stderr_sha256":hex::encode(Sha256::digest(&result.stderr)),
        "stdin_written":result.input.written,"stdin_complete":result.input.completed,"stdin_error":result.input.error,
        "jobs_empty":true,"profiles_removed":true,"scratch_removed":true,"scope":"native_process_host","production_execution":false}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> Request {
        Request {
            schema: 1,
            resource_id: "a".repeat(32),
            owner: Owner {
                identity: Binding {
                    protocol: 2,
                    installation_id: "b".repeat(32),
                    boot_id: format!("windows:{}:1", "c".repeat(32)),
                    package_sha256: "d".repeat(64),
                },
                run_id: "run".into(),
                owner_id: "controller".into(),
                epoch: 1,
            },
            program: Reference {
                path: "files/target.exe".into(),
                id: "e".repeat(64),
                sha256: "f".repeat(64),
                size: 42,
                executable: true,
            },
            arguments: vec!["--echo".into()],
            input: None,
            environment: BTreeMap::new(),
            timeout_secs: 5,
            output_limit: 65536,
            processes: 8,
            memory_bytes: 256 * 1024 * 1024,
        }
    }
    #[test]
    fn ordinary_tokens_cannot_enter_the_system_process_host() -> Result<()> {
        let root = tempfile::tempdir()?;
        let error = execute(
            &"a".repeat(32),
            &root.path().join("not-configured"),
            &root.path().join("not-published"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("System"));
        assert_eq!(fs::read_dir(root.path())?.count(), 0);
        Ok(())
    }
    #[test]
    fn process_requests_reject_unknown_fields_invalid_limits_and_control_environment() -> Result<()>
    {
        request().validate()?;
        for (key, value) in [
            ("schema", serde_json::json!(2)),
            ("resource_id", serde_json::json!("../outside")),
            ("timeout_secs", serde_json::json!(0)),
            ("processes", serde_json::json!(65)),
            ("memory_bytes", serde_json::json!(1)),
            ("output_limit", serde_json::json!(9 * 1024 * 1024)),
            ("environment", serde_json::json!({"PATH":"unapproved"})),
            ("arguments", serde_json::json!(["bad\u{0000}arg"])),
        ] {
            let mut data = serde_json::to_value(request())?;
            data[key] = value;
            assert!(serde_json::from_value::<Request>(data)?.validate().is_err());
        }
        let mut data = serde_json::to_value(request())?;
        data["pid"] = serde_json::json!(4);
        assert!(serde_json::from_value::<Request>(data).is_err());
        let mut allowed = request();
        allowed
            .environment
            .insert("ASAN_OPTIONS".into(), "abort_on_error=1".into());
        allowed.validate()?;
        Ok(())
    }
}
