//! Recover a create interrupted before any payload/service activation. Existing
//! installations use lifecycle repair, never this narrowly scoped account path.
use crate::{
    authority::{Authority, SignedEnrollment},
    deployment::{self, read_bytes},
    installer::Plan,
    security::AclPolicy,
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{fs, path::Path};
use wukong_worker_package::{PublicKey, SignatureRecord, VerifiedEnrollment, VerifiedManifest};
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
    let stored_authority = if metadata.join("authority.json").try_exists()? {
        Some(Authority::load(&metadata, &policy)?)
    } else {
        None
    };
    let key = if let Some(authority) = &stored_authority {
        let key = authority.key()?;
        ensure!(
            key.to_hex() == plan.setup.controller_public_key,
            "incoming setup cannot replace the protected first-owner authority"
        );
        key
    } else {
        // Interrupted development bootstrap with only the original intent:
        // the native owner fingerprint review still establishes first trust.
        PublicKey::from_hex(&plan.setup.controller_public_key)?
    };
    let proof = plan.base.join("recovery");
    let signature: SignatureRecord =
        serde_json::from_slice(&read_bytes(&proof.join("manifest.sig"), 1024)?)?;
    let previous = VerifiedManifest::verify(
        &read_bytes(&proof.join("manifest.json"), 4 * 1024 * 1024)?,
        &signature,
        &key,
    )?;
    let signed_prior = SignedEnrollment::read(&proof, &key)?;
    let prior = signed_prior.verify(&key)?;
    authorize(&intent, &previous, &prior, &plan.enrollment)?;
    let authority_exists = stored_authority.is_some();
    let authority = match stored_authority {
        Some(authority) => authority,
        None => Authority::new(signed_prior, &key)?,
    };
    authority.check_identity(&prior)?;
    for entry in fs::read_dir(&metadata)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_str().context("invalid recovery metadata name")?;
        if name == "installation-intent.json" || name == "authority.json" {
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
    }
    let highest = authority.highest(&metadata, &policy)?;
    let retry = highest.enrollment().generation == plan.enrollment.enrollment().generation;
    if retry {
        ensure!(
            highest.enrollment().canonical_bytes()?
                == plan.enrollment.enrollment().canonical_bytes()?,
            "recovery generation was already accepted with different contents"
        );
    } else {
        plan.enrollment.authorize_after(&highest)?;
    }
    let admission = SignedEnrollment::read(&plan.base.join("enrollment"), &key)?;
    admission.verify(&key)?;
    ensure!(
        admission.enrollment.canonical_bytes()?
            == plan.enrollment.enrollment().canonical_bytes()?,
        "recovery enrollment changed after review"
    );
    if !authority_exists {
        authority.persist(&metadata, &policy)?;
    }
    if retry {
        return Ok(());
    }
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
    use wukong_worker_package::{Compatibility, Enrollment, Signer, SshPublicKey, verify_package};
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

    fn signed(record: &VerifiedEnrollment, signer: &Signer) -> Result<SignedEnrollment> {
        let (bytes, signature) = record.enrollment().sign(signer)?;
        Ok(SignedEnrollment {
            enrollment: serde_json::from_slice(&bytes)?,
            signature,
        })
    }

    #[test]
    fn protected_authority_pins_the_signer_and_retains_failed_activation_admissions() -> Result<()>
    {
        let temporary = tempfile::tempdir()?;
        let source = temporary.path().join("original");
        let (signer, _) = Signer::generate()?;
        crate::staging::tests::inert_package(&source, &signer)?;
        let package = verify_package(
            &source,
            &signer.public_key(),
            &Compatibility {
                runtime_protocol: 2,
                controller_version: "1.0.0",
            },
        )?;
        let first = enrollment(&signer, &package, 2)?;
        let next = enrollment(&signer, &package, 3)?;
        let policy = AclPolicy::current_user()?;
        let metadata = temporary.path().join("protected-metadata");
        policy.create_directory(&metadata)?;
        let authority = Authority::new(signed(&first, &signer)?, &signer.public_key())?;
        authority.persist(&metadata, &policy)?;
        assert!(
            authority.persist(&metadata, &policy).is_err(),
            "first trust must never be overwritten"
        );
        let authority = Authority::load(&metadata, &policy)?;
        assert_eq!(authority.key()?.to_hex(), signer.public_key().to_hex());
        authority.verify_active(&metadata, &first, &policy)?;
        let (foreign, _) = Signer::generate()?;
        let replacement = enrollment(&foreign, &package, 3)?;
        assert_eq!(
            replacement.enrollment().payload_manifest_sha256,
            first.enrollment().payload_manifest_sha256
        );
        assert!(
            authority.check_identity(&replacement).is_err(),
            "a matching manifest hash does not authorize a new signer"
        );
        let stale = enrollment(&signer, &package, 1)?;
        assert!(authority.check_identity(&stale).is_err());
        deployment::write_new_json(
            &metadata.join("recovery-accepted-3.json"),
            &signed(&next, &signer)?,
            &policy,
        )?;
        // Simulate interruption after acceptance but before activation. Reloading
        // preserves the newer admission, so the original envelope cannot replay.
        let authority = Authority::load(&metadata, &policy)?;
        assert_eq!(
            authority
                .highest(&metadata, &policy)?
                .enrollment()
                .generation,
            3
        );
        assert!(authority.verify_active(&metadata, &first, &policy).is_err());
        authority.verify_active(&metadata, &next, &policy)?;
        deployment::write_new_json(
            &metadata.join("recovery-accepted-03.json"),
            &signed(&next, &signer)?,
            &policy,
        )?;
        assert!(authority.highest(&metadata, &policy).is_err());
        let path = metadata.join("authority.json");
        let mut corrupted: Value = serde_json::from_slice(&fs::read(&path)?)?;
        corrupted["first"]["enrollment"]["generation"] = json!(19);
        fs::write(path, serde_json::to_vec(&corrupted)?)?;
        assert!(Authority::load(&metadata, &policy).is_err());
        Ok(())
    }
}
