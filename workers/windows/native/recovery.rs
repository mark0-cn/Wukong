//! Recover a create interrupted before any payload/service activation. Existing
//! installations use lifecycle repair, never this narrowly scoped account path.
use crate::{
    deployment::{self, read_bytes},
    installer::Plan,
    security::AclPolicy,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{fs, path::Path};
use wukong_worker_package::{
    Enrollment, PublicKey, SignatureRecord, VerifiedEnrollment, VerifiedManifest,
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Admission {
    enrollment: Enrollment,
    signature: SignatureRecord,
}
impl Admission {
    fn verify(&self, key: &PublicKey) -> Result<VerifiedEnrollment> {
        VerifiedEnrollment::verify(&self.enrollment.canonical_bytes()?, &self.signature, key)
    }
}
fn authorize(
    intent: &Value,
    previous: &VerifiedManifest,
    prior: &VerifiedEnrollment,
    next: &VerifiedEnrollment,
) -> Result<()> {
    prior.bind_payload(previous)?;
    let record = prior.enrollment();
    ensure!(
        *intent
            == json!({"schema":1,"installation_id":record.installation_id,"manifest_sha256":previous.sha256(),"generation":record.generation,"roles":["management","worker","debugger","target"]}),
        "recovery does not match the protected original installation intent"
    );
    next.authorize_after(prior)
}
pub(crate) fn early_account_phase(root: &Path, plan: &Plan) -> Result<()> {
    let policy = AclPolicy::private_vm();
    policy.verify(root, true)?;
    let entries = fs::read_dir(root)?.collect::<std::io::Result<Vec<_>>>()?;
    ensure!(
        entries.len() == 1 && entries[0].file_name() == "metadata",
        "early recovery is forbidden after payload/data/service activation begins"
    );
    let metadata = root.join("metadata");
    policy.verify(&metadata, true)?;
    let intent_path = metadata.join("installation-intent.json");
    policy.verify(&intent_path, false)?;
    let intent: Value = serde_json::from_slice(&read_bytes(&intent_path, 16384)?)?;
    let key = PublicKey::from_hex(&plan.setup.controller_public_key)?;
    let proof = plan.base.join("recovery");
    let signature: SignatureRecord =
        serde_json::from_slice(&read_bytes(&proof.join("manifest.sig"), 1024)?)?;
    let previous = VerifiedManifest::verify(
        &read_bytes(&proof.join("manifest.json"), 4 * 1024 * 1024)?,
        &signature,
        &key,
    )?;
    let prior = deployment::read_enrollment(&proof, &key)?;
    authorize(&intent, &previous, &prior, &plan.enrollment)?;
    let mut admitted = Vec::new();
    for entry in fs::read_dir(&metadata)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_str().context("invalid recovery metadata name")?;
        if name == "installation-intent.json" {
            continue;
        }
        let generation = name
            .strip_prefix("recovery-accepted-")
            .and_then(|s| s.strip_suffix(".json"))
            .context("early account recovery refuses unrecognized metadata")?;
        ensure!(
            !generation.is_empty() && generation.bytes().all(|b| b.is_ascii_digit()),
            "invalid recovery admission filename"
        );
        policy.verify(&entry.path(), false)?;
        let admission: Admission = serde_json::from_slice(&read_bytes(&entry.path(), 32768)?)?;
        let admission = admission.verify(&key)?;
        ensure!(
            admission.enrollment().generation.to_string() == generation,
            "recovery admission generation differs from filename"
        );
        admission.authorize_after(&prior)?;
        admitted.push(admission);
        ensure!(
            admitted.len() <= 100,
            "too many pending native recovery admissions"
        );
    }
    admitted.sort_by_key(|a| a.enrollment().generation);
    if let Some(highest) = admitted.last() {
        if highest.enrollment().generation == plan.enrollment.enrollment().generation {
            ensure!(
                highest.enrollment().canonical_bytes()?
                    == plan.enrollment.enrollment().canonical_bytes()?,
                "recovery generation was already accepted with different contents"
            );
            return Ok(());
        }
        plan.enrollment.authorize_after(highest)?;
    }
    let signature: SignatureRecord = serde_json::from_slice(&read_bytes(
        &plan.base.join("enrollment/enrollment.sig"),
        1024,
    )?)?;
    let enrollment: Enrollment = serde_json::from_slice(&read_bytes(
        &plan.base.join("enrollment/enrollment.json"),
        16384,
    )?)?;
    let admission = Admission {
        enrollment,
        signature,
    };
    admission.verify(&key)?;
    deployment::write_new_json(
        &metadata.join(format!(
            "recovery-accepted-{}.json",
            admission.enrollment.generation
        )),
        &admission,
        &policy,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use wukong_worker_package::{Compatibility, Signer, SshPublicKey, verify_package};
    fn enrollment(
        signer: &Signer,
        package: &VerifiedManifest,
        generation: u64,
    ) -> Result<VerifiedEnrollment> {
        use base64::Engine;
        let mut blob = vec![0, 0, 0, 11];
        blob.extend_from_slice(b"ssh-ed25519");
        blob.extend_from_slice(&[0, 0, 0, 32]);
        blob.extend_from_slice(&[7; 32]);
        let record = Enrollment {
            schema: 1,
            generation,
            installation_id: "a".repeat(32),
            environment_id: "windows-vm".into(),
            controller_public_key: signer.public_key().to_hex(),
            payload_manifest_sha256: package.sha256().into(),
            management_peer: "192.0.2.1".parse()?,
            ssh_port: 2222,
            ssh_public_key: SshPublicKey {
                algorithm: "ssh-ed25519".into(),
                key: base64::engine::general_purpose::STANDARD.encode(blob),
            },
        };
        let (bytes, signature) = record.sign(signer)?;
        VerifiedEnrollment::verify(&bytes, &signature, &signer.public_key())
    }
    #[test]
    fn recovery_requires_original_protected_manifest_identity_and_a_higher_signed_generation()
    -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let source = temporary.path().join("original");
        let (signer, _) = Signer::generate()?;
        crate::staging::tests::inert_package(&source, &signer)?;
        let previous = verify_package(
            &source,
            &signer.public_key(),
            &Compatibility {
                runtime_protocol: 2,
                controller_version: "1.0.0",
            },
        )?;
        let prior = enrollment(&signer, &previous, 2)?;
        let next = enrollment(&signer, &previous, 3)?;
        let intent = json!({"schema":1,"installation_id":"a".repeat(32),"manifest_sha256":previous.sha256(),"generation":2,"roles":["management","worker","debugger","target"]});
        authorize(&intent, &previous, &prior, &next)?;
        assert!(authorize(&intent, &previous, &prior, &prior).is_err());
        let mut changed = intent;
        changed["manifest_sha256"] = json!("f".repeat(64));
        assert!(authorize(&changed, &previous, &prior, &next).is_err());
        Ok(())
    }
}
