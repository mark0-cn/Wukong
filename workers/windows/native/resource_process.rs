//! One System child per native execution resource. Controller admission owns
//! the fence and journal; this host accepts only its protected fixed request.
use crate::{
    deployment,
    inputs::digest,
    process_capture::{CaptureResult, PreparedIo},
    processes::ResourceLimits,
    resource_input::Reference,
    resource_journal::{Binding, Owner},
    security::{self, AclPolicy},
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io::Read, path::Path, time::Duration};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub schema: u32,
    pub resource_id: String,
    pub owner: Owner,
    pub program: Reference,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub argument_files: Vec<Reference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch: Option<crate::resource_launch::Binding>,
    pub arguments: Vec<String>,
    pub input: Option<Reference>,
    pub environment: BTreeMap<String, String>,
    pub timeout_secs: u64,
    pub output_limit: usize,
    pub processes: u32,
    pub memory_bytes: u64,
}
pub(crate) const SCHEMA: u32 = 3;
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScratchIntent {
    pub schema: u32,
    pub installation_id: String,
    pub resource_id: String,
    pub appcontainer_sid: String,
    pub logon_sid: String,
}
pub(crate) fn nonce(installation: &str, resource: &str) -> Result<String> {
    crate::processes::resource_job::name(installation, resource)?;
    Ok(digest(&("native.process.persona.v1", installation, resource))?[..48].to_owned())
}
impl Request {
    pub fn validate(&self) -> Result<()> {
        crate::resource_journal::bounded_identifier(&self.owner.run_id, 128)?;
        crate::resource_journal::bounded_identifier(&self.owner.owner_id, 128)?;
        ensure!(
            self.schema == SCHEMA
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
        if let Some(launch) = &self.launch {
            ensure!(
                self.argument_files.is_empty(),
                "LaunchSpec cannot override its file set with argv files"
            );
            launch.validate(&self.program, self.input.as_ref())?;
        } else {
            crate::resource_image::validate(&self.program, &self.argument_files, &self.arguments)?;
        }
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
        ensure!(
            serde_json::to_vec(self)?.len() <= 65536,
            "native process request exceeds protected record bound"
        );
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
    let host = crate::resource_host::open(installation, configuration)?;
    let root = &host.root;
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
            && request.owner.identity.package_sha256 == host.manifest.sha256(),
        "native process request belongs to another signed deployment"
    );
    crate::processes::resource_job::require_membership(installation, &request.resource_id)?;
    let cancel = root
        .join("metadata/resource-requests")
        .join(format!("{}.cancel", request.resource_id));
    if cancel.try_exists()? {
        return finish(
            &request,
            not_started(&request),
            String::new(),
            String::new(),
        );
    }
    let mut target = crate::resource_target::Prepared::open(&host, &request, None)?;
    let appcontainer = target.persona.sid_text()?;
    let logon = target.persona.logon_sid().to_owned();
    if cancel.try_exists()? {
        target.close()?;
        return finish(&request, not_started(&request), appcontainer, logon);
    }
    let limits = ResourceLimits::new(request.processes, request.memory_bytes)?;
    let input = target
        .input
        .take()
        .map(|input| Box::new(input) as Box<dyn Read + Send>);
    let process =
        PreparedIo::new(request.output_limit)?.launch(input, |io| target.launch(io, limits))?;
    let result = process.wait_with_cancel(Duration::from_secs(request.timeout_secs), || {
        Ok(cancel.try_exists()?)
    })?;
    target.close()?;
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
    deployment::publish_json(&directory.join(format!("{id}.json")), &value, &policy)?;
    Ok(serde_json::json!({"resource_id":id,"settled":true}))
}

fn not_started(request: &Request) -> CaptureResult {
    CaptureResult {
        pid: 0,
        exit_code: 1,
        reason: crate::process_capture::StopReason::Cancelled,
        stdout: Vec::new(),
        stderr: Vec::new(),
        input: crate::process_capture::InputDelivery {
            written: 0,
            completed: request.input.is_none(),
            error: request
                .input
                .as_ref()
                .map(|_| "cancelled before execution".into()),
        },
    }
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
        serde_json::json!({"schema":1,"resource_id":request.resource_id,"owner":request.owner,"program":request.program,"argument_files":request.argument_files,"launch":request.launch,"input":request.input,"request_sha256":digest(request)?,
        "pid":result.pid,"execution_started":result.pid != 0,"exit_code":result.exit_code,"stop_reason":reason,"appcontainer_sid":appcontainer,"logon_sid":logon,
        "stdout_base64":base64::engine::general_purpose::STANDARD.encode(&result.stdout),"stderr_base64":base64::engine::general_purpose::STANDARD.encode(&result.stderr),
        "stdout_sha256":hex::encode(Sha256::digest(&result.stdout)),"stderr_sha256":hex::encode(Sha256::digest(&result.stderr)),
        "stdin_written":result.input.written,"stdin_complete":result.input.completed,"stdin_error":result.input.error,
        "jobs_empty":true,"profiles_removed":true,"scratch_removed":true,"scope":"native_process_host","production_execution":false}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    fn request() -> Request {
        Request {
            schema: SCHEMA,
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
            argument_files: Vec::new(),
            launch: None,
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
            ("schema", serde_json::json!(SCHEMA + 1)),
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
        let mut oversized = request();
        oversized.environment = (0..64)
            .map(|index| (format!("AFL_{index}"), "x".repeat(4096)))
            .collect();
        assert!(oversized.validate().is_err());
        Ok(())
    }
}
