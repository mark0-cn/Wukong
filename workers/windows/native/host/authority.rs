//! Durable first-owner authority and accepted enrollment high-water marks.
//! Incoming media cannot choose a replacement for an already protected key.
use crate::isolation::security::AclPolicy;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::Path};
use wukong_worker_package::{Enrollment, PublicKey, SignatureRecord, VerifiedEnrollment};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SignedEnrollment {
    pub enrollment: Enrollment,
    pub signature: SignatureRecord,
}
impl SignedEnrollment {
    pub fn read(directory: &Path, key: &PublicKey) -> Result<Self> {
        let record = Self {
            enrollment: serde_json::from_slice(&crate::artifacts::io::read_bytes(
                &directory.join("enrollment.json"),
                16384,
            )?)?,
            signature: serde_json::from_slice(&crate::artifacts::io::read_bytes(
                &directory.join("enrollment.sig"),
                1024,
            )?)?,
        };
        record.verify(key)?;
        Ok(record)
    }
    pub fn verify(&self, key: &PublicKey) -> Result<VerifiedEnrollment> {
        VerifiedEnrollment::verify(&self.enrollment.canonical_bytes()?, &self.signature, key)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Authority {
    schema: u32,
    first: SignedEnrollment,
}
impl Authority {
    pub fn new(first: SignedEnrollment, reviewed_key: &PublicKey) -> Result<Self> {
        first.verify(reviewed_key)?;
        Ok(Self { schema: 1, first })
    }
    pub fn load(metadata: &Path, policy: &AclPolicy) -> Result<Self> {
        policy.verify(metadata, true)?;
        let path = metadata.join("authority.json");
        policy.verify(&path, false)?;
        let authority: Self =
            serde_json::from_slice(&crate::artifacts::io::read_bytes(&path, 32768)?)?;
        authority.key()?;
        Ok(authority)
    }
    pub fn persist(&self, metadata: &Path, policy: &AclPolicy) -> Result<()> {
        self.key()?;
        crate::artifacts::io::write_new_json(&metadata.join("authority.json"), self, policy)
    }
    pub fn key(&self) -> Result<PublicKey> {
        ensure!(
            self.schema == 1,
            "unsupported protected installation authority"
        );
        let key = PublicKey::from_hex(&self.first.enrollment.controller_public_key)?;
        self.first.verify(&key)?;
        Ok(key)
    }
    pub fn check_identity(&self, record: &VerifiedEnrollment) -> Result<()> {
        let first = self.first.verify(&self.key()?)?;
        if record.enrollment().generation == first.enrollment().generation {
            ensure!(
                record.enrollment().canonical_bytes()? == first.enrollment().canonical_bytes()?,
                "enrollment differs from the protected first-owner authority"
            );
            Ok(())
        } else {
            record.authorize_after(&first)
        }
    }
    pub fn highest(&self, metadata: &Path, policy: &AclPolicy) -> Result<VerifiedEnrollment> {
        let key = self.key()?;
        let mut highest = self.first.verify(&key)?;
        let mut admissions = BTreeMap::new();
        for entry in fs::read_dir(metadata)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_str().context("invalid protected metadata name")?;
            let Some(generation) = name.strip_prefix("recovery-accepted-") else {
                continue;
            };
            let generation = generation
                .strip_suffix(".json")
                .context("invalid protected admission filename")?;
            let number = generation
                .parse::<u64>()
                .context("invalid protected admission generation")?;
            ensure!(
                number.to_string() == generation,
                "noncanonical protected admission generation"
            );
            policy.verify(&entry.path(), false)?;
            let admission: SignedEnrollment =
                serde_json::from_slice(&crate::artifacts::io::read_bytes(&entry.path(), 32768)?)?;
            let admission = admission.verify(&key)?;
            ensure!(
                admission.enrollment().generation == number,
                "admission generation differs from its filename"
            );
            ensure!(
                admissions.insert(number, admission).is_none(),
                "duplicate protected admission generation"
            );
            ensure!(
                admissions.len() <= 100,
                "too many pending native recovery admissions"
            );
        }
        for admission in admissions.into_values() {
            admission.authorize_after(&highest)?;
            highest = admission;
        }
        Ok(highest)
    }
    pub fn verify_active(
        &self,
        metadata: &Path,
        record: &VerifiedEnrollment,
        policy: &AclPolicy,
    ) -> Result<()> {
        self.check_identity(record)?;
        ensure!(
            self.highest(metadata, policy)?
                .enrollment()
                .canonical_bytes()?
                == record.enrollment().canonical_bytes()?,
            "active enrollment differs from the protected accepted high-water mark"
        );
        Ok(())
    }
}
