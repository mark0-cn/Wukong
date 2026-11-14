//! Immutable resource outputs. The manager supplies an authorized journal record.
use crate::{
    executors::specification::Specification,
    isolation::security::AclPolicy,
    resources::models::{Phase, ResourceKind, ResourceRecord},
};
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub(crate) struct Artifacts {
    pub(crate) root: PathBuf,
}
impl Artifacts {
    pub(crate) fn retain(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        let policy = AclPolicy::private_vm();
        if path.try_exists()? {
            policy.verify(path, false)?;
            ensure!(
                crate::artifacts::io::read_bytes(path, bytes.len() as u64)? == bytes,
                "native published artifact cannot be replaced"
            );
            Ok(())
        } else {
            policy.publish(path, bytes)
        }
    }

    pub(crate) fn request_path(&self, id: &str, suffix: &str) -> Result<PathBuf> {
        validate_slot(id, suffix)?;
        Ok(self
            .root
            .join("metadata/resource-requests")
            .join(format!("{id}.{suffix}")))
    }

    pub(crate) fn result_path(&self, id: &str, suffix: &str) -> Result<PathBuf> {
        validate_slot(id, suffix)?;
        Ok(self
            .root
            .join("metadata/resource-results")
            .join(format!("{id}.{suffix}")))
    }

    pub(crate) fn artifact(&self, id: &str, name: &str) -> Result<Value> {
        let suffix = artifact_suffix(name)?;
        let path = self.result_path(id, &suffix)?;
        AclPolicy::private_vm().verify(&path, false)?;
        let data = crate::artifacts::io::read_bytes(&path, 32 * 1024 * 1024)?;
        Ok(
            json!({"path":format!("native-results/{id}/{name}"),"size":data.len(),"sha256":hex::encode(Sha256::digest(&data))}),
        )
    }

    pub(crate) fn read(
        &self,
        record: &ResourceRecord,
        method: &str,
        params: &Map<String, Value>,
    ) -> Result<Value> {
        let path = params["path"]
            .as_str()
            .context("native artifact selector is required")?;
        let parts: Vec<_> = path.split('/').collect();
        ensure!(
            parts.len() == 3 && parts[0] == "native-results",
            "invalid native artifact selector"
        );
        ensure!(
            record.id == parts[1],
            "artifact selector differs from its authorized resource"
        );
        let data = if let Some(sequence) = parts[2].strip_prefix("transcript-") {
            ensure!(
                matches!(
                    record.kind,
                    ResourceKind::Debugger | ResourceKind::Interactive
                ),
                "native transcript requires its session resource"
            );
            let parsed: u64 = sequence.parse()?;
            ensure!(
                parsed.to_string() == sequence,
                "noncanonical native transcript sequence"
            );
            let prefix = crate::executors::session_request::prefix(record.kind)?;
            let request_path = self.request_path(parts[1], &format!("{prefix}.json"))?;
            AclPolicy::private_vm().verify(&request_path, false)?;
            let request: crate::executors::session_request::Request =
                serde_json::from_slice(&crate::artifacts::io::read_bytes(&request_path, 65536)?)?;
            request.validate()?;
            let spec: Specification =
                serde_json::from_value(record.specification["process"].clone())?;
            ensure!(
                request.target.resource_id == record.id
                    && request.target.owner == record.owner
                    && spec.matches(&request.target)
                    && request.kind() == record.kind
                    && if record.kind == ResourceKind::Interactive {
                        record.specification["native_interactive_schema"] == request.schema
                    } else {
                        record.specification["native_debugger_schema"] == request.schema
                            && record.specification["debugger"]["mode"]
                                == if request.read_only() {
                                    "dump"
                                } else {
                                    "launch"
                                }
                            && record.specification["debugger"]["dump"].as_str()
                                == request.dump().map(|file| file.path.as_str())
                    },
                "native debugger transcript request differs from its journal intent"
            );
            crate::artifacts::session_artifacts::read(
                &self.root,
                parts[1],
                parsed,
                &crate::artifacts::inputs::digest(&request)?,
            )?
        } else {
            ensure!(
                record.phase == Phase::Closed,
                "native artifacts require completed cleanup"
            );
            let suffix = if parts[2] == "receipt" && record.kind != ResourceKind::Process {
                let status = self.session_terminal(record, &AclPolicy::private_vm())?;
                if status["state"] == "lost" {
                    artifact_suffix(
                        status["result"]["recovery_artifact"]
                            .as_str()
                            .context("debugger recovery selector missing")?,
                    )?
                } else {
                    format!(
                        "{}-result.json",
                        crate::executors::session_request::prefix(record.kind)?
                    )
                }
            } else {
                artifact_suffix(parts[2])?
            };
            let source = self.result_path(parts[1], &suffix)?;
            AclPolicy::private_vm().verify(&source, false)?;
            crate::artifacts::io::read_bytes(&source, 32 * 1024 * 1024)?
        };
        let mut artifact =
            json!({"path":path,"size":data.len(),"sha256":hex::encode(Sha256::digest(&data))});
        if method == "file.stat" {
            return Ok(artifact);
        }
        if method == "file.get" {
            artifact["data_base64"] = json!(STANDARD.encode(data));
            return Ok(artifact);
        }
        let integer = |key: &str, default: u64| -> Result<usize> {
            Ok(usize::try_from(match params.get(key) {
                None => default,
                Some(value) => value
                    .as_u64()
                    .context("native read offset/limit must be an unsigned integer")?,
            })?)
        };
        let offset = integer("offset", 0)?;
        let maximum = integer("max_bytes", 1024 * 1024)?;
        ensure!(
            offset <= data.len() && (1..=1024 * 1024).contains(&maximum),
            "native read range exceeds bounds"
        );
        let chunk = &data[offset..data.len().min(offset.saturating_add(maximum))];
        Ok(
            json!({"path":path,"offset":offset,"size":data.len(),"data_base64":STANDARD.encode(chunk),"chunk_sha256":hex::encode(Sha256::digest(chunk))}),
        )
    }

    pub(crate) fn session_terminal(
        &self,
        record: &ResourceRecord,
        policy: &AclPolicy,
    ) -> Result<Value> {
        ensure!(
            matches!(
                record.kind,
                ResourceKind::Debugger | ResourceKind::Interactive
            ) && record.phase == Phase::Closed,
            "debugger terminal observation requires confirmed closure"
        );
        let status_path = self.result_path(&record.id, "status.json")?;
        if status_path.try_exists()? {
            policy.verify(&status_path, false)?;
            let value: Value =
                serde_json::from_slice(&crate::artifacts::io::read_bytes(&status_path, 65536)?)?;
            ensure!(
                value["session_id"] == record.id
                    && value["worker_session_id"] == record.id
                    && value["native_owner"] == serde_json::to_value(&record.owner)?
                    && value["kind"] == serde_json::to_value(record.kind)?
                    && value["isolation"] == "appcontainer"
                    && value["state"] == "lost"
                    && value["cleanup_pending"] == false
                    && value["result"]["outcome"] == "unknown"
                    && value["result"]["jobs_empty"] == true
                    && value["result"]["profiles_removed"] == true
                    && value["result"]["scratch_removed"] == true,
                "native recovered debugger status binding or cleanup differs"
            );
            let artifact = value["result"]["recovery_artifact"]
                .as_str()
                .context("debugger recovery artifact missing")?;
            ensure!(
                artifact.starts_with("recovery-"),
                "debugger recovery selector is not a recovery receipt"
            );
            let proof_path = self.result_path(&record.id, &artifact_suffix(artifact)?)?;
            policy.verify(&proof_path, false)?;
            let bytes = crate::artifacts::io::read_bytes(&proof_path, 65536)?;
            let proof: Value = serde_json::from_slice(&bytes)?;
            ensure!(
                value["result"]["recovery_receipt_sha256"] == hex::encode(Sha256::digest(&bytes))
                    && proof["schema"] == crate::executors::resource_process::SCHEMA
                    && proof["scope"] == "owned_resource_recovery"
                    && proof["resource_id"] == record.id
                    && proof["owner"] == serde_json::to_value(&record.owner)?
                    && proof["jobs_empty"] == true
                    && proof["profiles_removed"] == true
                    && proof["scratch_removed"] == true,
                "debugger recovery receipt content or binding differs"
            );
            return Ok(value);
        }
        let prefix = crate::executors::session_request::prefix(record.kind)?;
        let proof_path = self.result_path(&record.id, &format!("{prefix}-result.json"))?;
        policy.verify(&proof_path, false)?;
        let proof: Value =
            serde_json::from_slice(&crate::artifacts::io::read_bytes(&proof_path, 65536)?)?;
        ensure!(
            proof["schema"] == crate::executors::session_host::SCHEMA
                && proof["scope"]
                    == if record.kind == ResourceKind::Interactive {
                        "native_interactive_host"
                    } else {
                        "native_debugger_host"
                    }
                && proof["resource_id"] == record.id
                && proof["owner"] == serde_json::to_value(&record.owner)?
                && proof["outcome"] == "closed"
                && proof["jobs_empty"] == true
                && proof["profiles_removed"] == true
                && proof["scratch_removed"] == true,
            "native closed debugger proof binding or cleanup differs"
        );
        Ok(
            json!({"session_id":record.id,"worker_session_id":record.id,"native_owner":record.owner,
            "kind":record.kind,"state":"closed","cleanup_pending":false,"result":proof,"isolation":"appcontainer"}),
        )
    }
}

fn validate_slot(id: &str, suffix: &str) -> Result<()> {
    ensure!(
        id.len() == 32
            && id
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "invalid resource artifact identity"
    );
    ensure!(
        !suffix.is_empty()
            && suffix.len() <= 128
            && !suffix.starts_with('.')
            && suffix
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_')),
        "invalid resource artifact suffix"
    );
    Ok(())
}

fn artifact_suffix(name: &str) -> Result<String> {
    if let Some(trial) = name.strip_prefix("trial-") {
        let (trial, kind) = trial.split_once('-').unwrap_or((trial, "record"));
        let index: u16 = trial.parse()?;
        ensure!(
            index < 4096 && index.to_string() == trial,
            "invalid replay artifact index"
        );
        let suffix = match kind {
            "record" => "json",
            "input" => "input",
            "native" => "native.json",
            "mcp" => "mcp",
            "observation" => "observation.json",
            "debug-log" => "debug-log",
            "debug-stdout" => "debug-stdout",
            "debug-stderr" => "debug-stderr",
            _ => anyhow::bail!("invalid minimization artifact kind"),
        };
        return Ok(format!("trial-{index}.{suffix}"));
    }
    match name {
        "stdout" | "stderr" | "minimized" => Ok(name.into()),
        "receipt" => Ok("json".into()),
        _ => {
            let attempt = name
                .strip_prefix("recovery-")
                .context("unknown native artifact kind")?;
            ensure!(
                attempt.len() == 32
                    && attempt
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
                "invalid native recovery artifact identity"
            );
            Ok(format!("{name}.json"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::state::tests::owner;
    use std::fs;
    #[test]
    fn terminal_session_observation_preserves_unknown_outcomes_and_binds_receipt_bytes()
    -> Result<()> {
        for kind in [ResourceKind::Debugger, ResourceKind::Interactive] {
            let directory = tempfile::tempdir()?;
            let policy = AclPolicy::current_user()?;
            policy.apply(directory.path(), true)?;
            policy.create_directory(&directory.path().join("metadata"))?;
            policy.create_directory(&directory.path().join("metadata/resource-results"))?;
            let runtime = crate::artifacts::outputs::Artifacts {
                root: directory.path().into(),
            };
            let record = ResourceRecord {
                id: "d".repeat(32),
                phase: Phase::Closed,
                start_effect: "e".repeat(64),
                owner: owner(),
                kind,
                specification: json!({}),
            };
            let artifact = format!("recovery-{}", "f".repeat(32));
            let proof = json!({"schema":crate::executors::resource_process::SCHEMA,"scope":"owned_resource_recovery",
            "resource_id":record.id,"owner":record.owner,"jobs_empty":true,"profiles_removed":true,"scratch_removed":true});
            let proof_path = runtime.result_path(&record.id, &artifact_suffix(&artifact)?)?;
            crate::artifacts::io::publish_json(&proof_path, &proof, &policy)?;
            let original = fs::read(&proof_path)?;
            let status = json!({"session_id":record.id,"worker_session_id":record.id,"native_owner":record.owner,"kind":kind,"isolation":"appcontainer",
            "state":"lost","cleanup_pending":false,"result":{"outcome":"unknown","jobs_empty":true,
            "profiles_removed":true,"scratch_removed":true,"recovery_artifact":artifact,
            "recovery_receipt_sha256":hex::encode(Sha256::digest(&original))}});
            let status_path = runtime.result_path(&record.id, "status.json")?;
            crate::artifacts::io::publish_json(&status_path, &status, &policy)?;
            assert_eq!(runtime.session_terminal(&record, &policy)?, status);
            let mut foreign_epoch = record.clone();
            foreign_epoch.owner.epoch += 1;
            let mut foreign_run = record.clone();
            foreign_run.owner.run_id = "another-run".into();
            let mut foreign_resource = record.clone();
            foreign_resource.id = "0".repeat(32);
            for changed in [&foreign_epoch, &foreign_run, &foreign_resource] {
                assert!(runtime.session_terminal(changed, &policy).is_err());
            }
            let mut live = record.clone();
            live.phase = Phase::Closing;
            assert!(runtime.session_terminal(&live, &policy).is_err());
            let mut process = record.clone();
            process.kind = ResourceKind::Process;
            assert!(runtime.session_terminal(&process, &policy).is_err());
            let mut altered = proof.clone();
            altered["owner"]["epoch"] = json!(99);
            fs::write(&proof_path, serde_json::to_vec(&altered)?)?;
            assert!(runtime.session_terminal(&record, &policy).is_err());
            fs::write(&proof_path, &original)?;
            for (field, value) in [
                ("outcome", json!("closed")),
                ("jobs_empty", json!(false)),
                ("recovery_artifact", json!("stdout")),
                ("recovery_artifact", json!("recovery-../other")),
            ] {
                let mut changed = status.clone();
                changed["result"][field] = value;
                fs::write(&status_path, serde_json::to_vec(&changed)?)?;
                assert!(
                    runtime.session_terminal(&record, &policy).is_err(),
                    "{field}"
                );
            }
            fs::remove_file(&status_path)?;
            assert!(runtime.session_terminal(&record, &policy).is_err());
            let clean = json!({"schema":crate::executors::session_host::SCHEMA,"scope":if kind == ResourceKind::Debugger {"native_debugger_host"} else {"native_interactive_host"},
            "resource_id":record.id,"owner":record.owner,"outcome":"closed",
            "jobs_empty":true,"profiles_removed":true,"scratch_removed":true});
            crate::artifacts::io::publish_json(
                &runtime.result_path(
                    &record.id,
                    &format!(
                        "{}-result.json",
                        crate::executors::session_request::prefix(kind)?
                    ),
                )?,
                &clean,
                &policy,
            )?;
            let closed = runtime.session_terminal(&record, &policy)?;
            assert_eq!(closed["state"], "closed");
            assert_eq!(closed["result"], clean);
            assert_eq!(fs::read(&proof_path)?, original);
        }
        Ok(())
    }
}
