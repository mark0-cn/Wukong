//! Reconcile only a System-private resource intent; never execute its target.
use crate::{
    accounts::Role,
    deployment,
    inputs::digest,
    resource_journal::{ResourceKind, ResourceRecord},
    resource_process::{self, ScratchIntent},
    security::{self, AclPolicy},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{path::Path, time::Duration};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Request {
    pub schema: u32,
    pub attempt: String,
    pub resource: ResourceRecord,
}
impl Request {
    pub(crate) fn validate(&self) -> Result<()> {
        let record = &self.resource;
        ensure!(
            self.schema == resource_process::SCHEMA
                && self.attempt.len() == 32
                && self
                    .attempt
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
            "invalid native recovery attempt"
        );
        resource_process::nonce(&record.owner.identity.installation_id, &record.id)?;
        ensure!(
            record.phase != crate::resource_journal::Phase::Closed
                && record.specification["process"].is_object()
                && match record.kind {
                    ResourceKind::Process =>
                        record.specification["native_process_schema"] == resource_process::SCHEMA,
                    ResourceKind::Debugger =>
                        record.specification["native_debugger_schema"]
                            == crate::session_host::SCHEMA
                            && record.specification["debugger"].is_object(),
                    ResourceKind::Interactive =>
                        record.specification["native_interactive_schema"]
                            == crate::session_host::SCHEMA,
                },
            "unsupported native resource recovery contract"
        );
        owned_personas(record)?;
        Ok(())
    }
}
fn owned_personas(record: &ResourceRecord) -> Result<Vec<(Role, String)>> {
    let installation = &record.owner.identity.installation_id;
    let replay: Option<crate::resource_replay::Configuration> =
        serde_json::from_value(record.specification["replay"].clone())?;
    let mut personas = if let Some(replay) = &replay {
        replay.validate()?;
        ensure!(
            record.kind == ResourceKind::Process && record.specification["interpreter"].is_null(),
            "replay recovery cannot adopt another resource kind"
        );
        (0..replay.trials)
            .map(|trial| {
                Ok((
                    Role::Target,
                    crate::resource_replay::nonce(installation, &record.id, trial)?,
                ))
            })
            .collect::<Result<Vec<_>>>()?
    } else {
        vec![(
            Role::Target,
            resource_process::nonce(installation, &record.id)?,
        )]
    };
    if let Some(minimize) = replay
        .as_ref()
        .and_then(|replay| replay.minimization.as_ref())
        && !minimize.allow_signal_only
    {
        for trial in 0..minimize.max_attempts {
            personas.push((
                Role::Target,
                crate::resource_minimize::target_nonce(installation, &record.id, trial)?,
            ));
            personas.push((
                Role::Debugger,
                crate::resource_minimize::debugger_nonce(installation, &record.id, trial)?,
            ));
        }
    }
    if record.kind == ResourceKind::Debugger {
        personas.push((
            Role::Debugger,
            crate::session_host::nonce(installation, &record.id)?,
        ));
    }
    Ok(personas)
}
pub fn run(installation: &str, configuration: &Path, request_path: &Path) -> Result<Value> {
    security::require_system()?;
    let crate::resource_host::Host {
        deployment,
        root,
        resources,
        ..
    } = crate::resource_host::open(installation, configuration)
        .context("open original resource recovery host")?;
    let private = AclPolicy::private_vm();
    private
        .verify(request_path, false)
        .context("verify protected resource recovery request")?;
    let request: Request = serde_json::from_slice(&deployment::read_bytes(request_path, 65536)?)?;
    request.validate()?;
    let record = &request.resource;
    let expected = root
        .join("metadata/resource-requests")
        .join(format!("{}.recovery-{}.json", record.id, request.attempt));
    ensure!(
        request_path.canonicalize()? == expected.canonicalize()?
            && record.owner.identity.installation_id == installation,
        "native recovery request slot or installation differs"
    );
    let found =
        crate::processes::resource_job::drain(installation, &record.id, Duration::from_secs(15))
            .context("drain original resource guardian")?;
    for (role, nonce) in owned_personas(record)? {
        let metadata = root.join("metadata");
        let profile_path = metadata.join(format!("resource-profile-{nonce}.json"));
        let scratch_path = resources.join(&nonce);
        let scratch_intent = metadata.join(format!("resource-scratch-{nonce}.json"));
        if profile_path.try_exists()? {
            private.verify(&profile_path, false)?;
            let profile: Value =
                serde_json::from_slice(&deployment::read_bytes(&profile_path, 8192)?)?;
            let name = format!("Wukong-{nonce}");
            ensure!(
                profile
                    == json!({"schema":1,"installation_id":installation,"role":role,"account_sid":deployment.account(role)?.sid,"profile_name":name,"resource_id":record.id}),
                "native recovery profile intent differs"
            );
            crate::persona::Persona::recover(installation, deployment.account(role)?, &name)
                .with_context(|| format!("recover original {role:?} persona"))?;
        } else {
            ensure!(
                !scratch_path.try_exists()? && !scratch_intent.try_exists()?,
                "scratch exists without its original profile intent"
            );
        }
        if scratch_path.try_exists()? {
            private.verify(&scratch_intent, false)?;
            let scratch: ScratchIntent =
                serde_json::from_slice(&deployment::read_bytes(&scratch_intent, 8192)?)?;
            ensure!(
                scratch.schema == resource_process::SCHEMA
                    && scratch.installation_id == installation
                    && scratch.resource_id == record.id,
                "native recovery scratch intent differs"
            );
            security::ResourceDirectory::recover(
                &scratch_path,
                &scratch.appcontainer_sid,
                &scratch.logon_sid,
            )
            .with_context(|| format!("recover original {role:?} scratch"))?;
        }
        ensure!(
            !scratch_path.try_exists()?,
            "native recovery scratch remains"
        );
    }
    let value = json!({"schema":resource_process::SCHEMA,"scope":"owned_resource_recovery","attempt":request.attempt,"resource_id":record.id,"record_sha256":digest(record)?,"owner":record.owner,"guardian_found":found,"jobs_empty":true,"profiles_removed":true,"scratch_removed":true,"production_execution":false});
    deployment::publish_json(
        &root
            .join("metadata/resource-results")
            .join(format!("{}.recovery-{}.json", record.id, request.attempt)),
        &value,
        &private,
    )?;
    Ok(json!({"resource_id":record.id,"reconciled":true}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ordinary_tokens_cannot_reconcile_profiles_jobs_or_scratch() -> Result<()> {
        let root = tempfile::tempdir()?;
        let error = run(
            &"a".repeat(32),
            &root.path().join("missing"),
            &root.path().join("missing-request"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("System"));
        assert_eq!(std::fs::read_dir(root.path())?.count(), 0);
        Ok(())
    }
}
