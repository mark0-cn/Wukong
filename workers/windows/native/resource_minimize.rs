//! Native execution/equivalence policy around the existing ddmin library.
//! Candidate bytes stay in protected evidence and enter only owned target Jobs.
use crate::{
    deployment,
    inputs::digest,
    resource_host::Host,
    resource_input::{self, AdmittedInput, Purpose, Reference},
    resource_process::{self, Request},
    resource_replay,
    security::{self, AclPolicy},
};
use anyhow::{Context, Result, ensure};
use fixture3_ddmin::{
    DdminGuarantee, DdminInput, DdminOptions, OracleOutcome, UnresolvedReason, ddmin,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    cell::Cell,
    fs,
    io::{Read, Seek, SeekFrom},
    num::NonZeroUsize,
    os::windows::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;

const MAX_INPUT: u64 = 32 * 1024 * 1024;
pub(crate) const EVIDENCE_LIMIT: u64 = 256 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    pub max_attempts: u16,
    pub timeout_secs: u64,
    pub allow_signal_only: bool,
}
impl Configuration {
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            (1..=4096).contains(&self.max_attempts) && (1..=3600).contains(&self.timeout_secs),
            "minimization requires 1..4096 attempts and 1..3600 seconds"
        );
        Ok(())
    }
}
pub(crate) fn target_nonce(installation: &str, resource: &str, index: u16) -> Result<String> {
    resource_replay::nonce(installation, resource, index)?;
    Ok(digest(&(
        "native.minimize.observed-target.v1",
        installation,
        resource,
        index,
    ))?[..48]
        .into())
}
pub(crate) fn debugger_nonce(installation: &str, resource: &str, index: u16) -> Result<String> {
    resource_replay::nonce(installation, resource, index)?;
    Ok(digest(&("native.minimize.debugger.v1", installation, resource, index))?[..48].into())
}
pub(crate) struct Candidate {
    pub reference: Reference,
    path: PathBuf,
}
impl Candidate {
    fn retain(host: &Host, request: &Request, index: u16, data: &[u8]) -> Result<Self> {
        let replay = request
            .replay
            .as_ref()
            .context("candidate has no replay plan")?;
        ensure!(
            data.len() as u64 <= replay.input.size && index < replay.configuration.trials,
            "candidate exceeds original input or attempt bound"
        );
        let path = result_path(host, request, &format!("trial-{index}.input"));
        AclPolicy::private_vm().publish(&path, data)?;
        let sha256 = hex::encode(Sha256::digest(data));
        Ok(Self {
            path,
            reference: Reference {
                path: replay.input.path.clone(),
                id: digest(&(
                    "native.minimize.candidate.v1",
                    &request.resource_id,
                    index,
                    &sha256,
                ))?,
                sha256,
                size: data.len() as u64,
                executable: false,
            },
        })
    }
    pub(crate) fn open(&self) -> Result<AdmittedInput> {
        security::no_links(&self.path)?;
        AclPolicy::private_vm().verify(&self.path, false)?;
        let mut file = fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&self.path)?;
        let mut data = Vec::new();
        (&mut file).take(MAX_INPUT + 1).read_to_end(&mut data)?;
        ensure!(
            data.len() as u64 == self.reference.size
                && hex::encode(Sha256::digest(&data)) == self.reference.sha256,
            "retained candidate bytes changed"
        );
        file.seek(SeekFrom::Start(0))?;
        Ok(AdmittedInput::new(file, self.reference.clone()))
    }
}
pub(crate) fn result_path(host: &Host, request: &Request, suffix: &str) -> PathBuf {
    host.root
        .join("metadata/resource-results")
        .join(format!("{}.{suffix}", request.resource_id))
}

fn trial(
    host: &Host,
    request: &Request,
    cancel: &Path,
    index: u16,
    data: &[u8],
    deadline: Instant,
) -> Result<Value> {
    let configuration = request
        .replay
        .as_ref()
        .unwrap()
        .configuration
        .minimization
        .as_ref()
        .unwrap();
    let candidate = Candidate::retain(host, request, index, data)?;
    let target = crate::resource_target::Prepared::open_candidate(
        host,
        request,
        index,
        candidate.open()?,
        None,
    )?;
    let native =
        resource_process::execute_prepared(request, cancel, Some(index), target, Some(deadline))?;
    resource_process::output(request, &native)?;
    let code = native["exit_code"]
        .as_u64()
        .context("native exit code missing")? as u32;
    let crashed = native["stop_reason"] == "exited" && resource_replay::signature(code).is_some();
    let observation = if crashed && !configuration.allow_signal_only {
        Some(crate::session_crash::observe(
            host, request, &candidate, index, code, cancel, deadline,
        )?)
    } else {
        None
    };
    let signature = if configuration.allow_signal_only && crashed {
        resource_replay::signature(code)
    } else {
        observation
            .as_ref()
            .and_then(|v| v["location_signature"].as_str())
            .map(str::to_owned)
    };
    let resolved = native["stop_reason"] == "exited" && (!crashed || signature.is_some());
    let value = json!({"schema":1,"trial":index,"resource_id":request.resource_id,"owner":request.owner,
        "request_sha256":digest(request)?,"input":candidate.reference,"native":native,
        "observation":observation,"crash_signature":signature,"crashed":crashed,"resolved":resolved});
    deployment::publish_json(
        &result_path(host, request, &format!("trial-{index}.json")),
        &value,
        &AclPolicy::private_vm(),
    )?;
    Ok(value)
}

/// The library's unresolved outcomes are not a completion guarantee. Once an
/// oracle becomes uncertain, no later callback may execute another candidate.
fn reduce(
    original: Vec<u8>,
    maximum: u16,
    mut evaluate: impl FnMut(&[u8], u16) -> Result<Option<bool>>,
    mut stop: impl FnMut() -> Result<Option<&'static str>>,
) -> Result<(Vec<u8>, u16, bool, Option<String>)> {
    let mut best = original.clone();
    let mut attempts = 1u16;
    let mut stopped = None;
    let mut failure = None;
    let mut oracle = |data: &[u8]| {
        if stopped.is_some() || failure.is_some() {
            return OracleOutcome::Unresolved(UnresolvedReason::OracleFailed);
        }
        match stop() {
            Ok(Some(reason)) => {
                stopped = Some(reason.to_owned());
                return OracleOutcome::Unresolved(UnresolvedReason::Timeout);
            }
            Err(error) => {
                failure = Some(error);
                return OracleOutcome::Unresolved(UnresolvedReason::OracleFailed);
            }
            Ok(None) => {}
        }
        if data == best {
            return OracleOutcome::Interesting;
        }
        if attempts == maximum {
            stopped = Some("attempt budget exhausted".into());
            return OracleOutcome::Unresolved(UnresolvedReason::Timeout);
        }
        let index = attempts;
        attempts += 1;
        match evaluate(data, index) {
            Ok(Some(true)) => {
                best = data.to_vec();
                OracleOutcome::Interesting
            }
            Ok(Some(false)) => OracleOutcome::NotInteresting,
            Ok(None) => {
                stopped = Some("candidate outcome unresolved".into());
                OracleOutcome::Unresolved(UnresolvedReason::OracleFailed)
            }
            Err(error) => {
                failure = Some(error);
                OracleOutcome::Unresolved(UnresolvedReason::OracleFailed)
            }
        }
    };
    // Explicit empty check also covers non-monotonic oracles whose singleton
    // deletions fail but empty input still reproduces the original failure.
    let empty = oracle(&[]) == OracleOutcome::Interesting;
    let guarantee = if empty {
        DdminGuarantee::OneMinimalWithinCandidateSet
    } else {
        // The extra call is the cached, already verified baseline. Even after
        // cancellation, library work is bounded and cannot execute more targets.
        ddmin(
            DdminInput::new(
                original,
                DdminOptions::new(
                    NonZeroUsize::new(2).unwrap(),
                    Some(usize::from(maximum) + 1),
                ),
            ),
            &mut oracle,
        )
        .guarantee()
    };
    if let Some(error) = failure {
        return Err(error);
    }
    if stopped.is_none() {
        stopped = stop()?.map(str::to_owned);
    }
    if stopped.is_none() && guarantee != DdminGuarantee::OneMinimalWithinCandidateSet {
        stopped = Some("reducer call budget exhausted".into());
    }
    Ok((best, attempts, stopped.is_none(), stopped))
}

pub(crate) fn execute(host: &Host, request: &Request, cancel: &Path) -> Result<Value> {
    let replay = request
        .replay
        .as_ref()
        .context("minimization requires replay")?;
    let configuration = replay
        .configuration
        .minimization
        .as_ref()
        .context("minimization settings absent")?;
    let deadline = Instant::now() + Duration::from_secs(configuration.timeout_secs);
    let mut original = Vec::new();
    resource_input::open(
        &host.root.join("metadata"),
        &request.owner,
        &replay.input,
        Purpose::Inspect,
    )?
    .take(MAX_INPUT + 1)
    .read_to_end(&mut original)?;
    ensure!(
        original.len() as u64 == replay.input.size,
        "original minimization input length differs"
    );
    let baseline = trial(host, request, cancel, 0, &original, deadline)?;
    let evidence_used = Cell::new(trial_size(host, request, 0)?);
    let signature = baseline["crash_signature"].as_str().map(str::to_owned);
    let mut trials = vec![trial_digest(host, request, 0)?];
    let mut failed = None;
    let (best, attempts, complete, stop_reason) = if let Some(signature) = &signature {
        reduce(
            original.clone(),
            configuration.max_attempts,
            |candidate, index| {
                let value = trial(host, request, cancel, index, candidate, deadline)?;
                evidence_used.set(evidence_used.get() + trial_size(host, request, index)?);
                trials.push(trial_digest(host, request, index)?);
                Ok((value["resolved"] == true)
                    .then(|| value["crash_signature"].as_str() == Some(signature)))
            },
            || {
                if cancel.try_exists()? {
                    Ok(Some("cancelled"))
                } else if Instant::now() >= deadline {
                    Ok(Some("deadline expired"))
                } else if evidence_used.get() + replay.input.size + 40 * 1024 * 1024
                    > EVIDENCE_LIMIT
                {
                    Ok(Some("evidence budget exhausted"))
                } else {
                    Ok(None)
                }
            },
        )?
    } else {
        failed = Some(if baseline["crashed"] == true {
            "location signature unavailable; inspect the original observation or explicitly opt into allow_signal_only"
        } else {
            "input did not produce a resolved native crash"
        });
        (
            original.clone(),
            1,
            false,
            Some("baseline is not a verified reproducer".into()),
        )
    };
    // Do not publish a minimized artifact when even the baseline is unverified.
    let minimized = if failed.is_none() {
        AclPolicy::private_vm().publish(&result_path(host, request, "minimized"), &best)?;
        Some(
            json!({"path":format!("native-results/{}/minimized",request.resource_id),
            "size":best.len(),"sha256":hex::encode(Sha256::digest(&best))}),
        )
    } else {
        None
    };
    Ok(
        json!({"schema":1,"scope":"native_minimize_host","resource_id":request.resource_id,
        "owner":request.owner,"request_sha256":digest(request)?,"trials":trials,
        "attempts":attempts,"original_size":original.len(),"minimized_size":failed.is_none().then_some(best.len()),
        "artifact":minimized,"crash_signature":signature,"complete":complete,"stop_reason":stop_reason,
        "reducer":"fixture3-ddmin",
        "equivalence_policy":if configuration.allow_signal_only {"signal_only"} else {"location_and_frames"},
        "signature_strength":if signature.is_none(){"unresolved"}else if configuration.allow_signal_only {"signal_only"} else {"location_and_frames"},
        "failed":failed,"cancelled":cancel.try_exists()?,"jobs_empty":true,
        "profiles_removed":true,"scratch_removed":true,"production_execution":false}),
    )
}
fn trial_digest(host: &Host, request: &Request, index: u16) -> Result<Value> {
    let data = deployment::read_bytes(
        &result_path(host, request, &format!("trial-{index}.json")),
        24 * 1024 * 1024,
    )?;
    Ok(
        json!({"trial":index,"sha256":hex::encode(Sha256::digest(&data)),
        "path":format!("native-results/{}/trial-{index}", request.resource_id),
        "input_path":format!("native-results/{}/trial-{index}-input", request.resource_id)}),
    )
}
fn trial_size(host: &Host, request: &Request, index: u16) -> Result<u64> {
    let mut bytes = 0u64;
    for suffix in [
        "input",
        "json",
        "mcp",
        "observation.json",
        "debug-log",
        "debug-stdout",
        "debug-stderr",
    ] {
        let path = result_path(host, request, &format!("trial-{index}.{suffix}"));
        if path.try_exists()? {
            bytes = bytes
                .checked_add(fs::metadata(path)?.len())
                .context("evidence size overflow")?;
        }
    }
    Ok(bytes)
}

pub(crate) fn summarize(
    request: &Request,
    receipt: &Value,
    mut read: impl FnMut(&str) -> Result<Vec<u8>>,
) -> Result<Value> {
    let replay = request
        .replay
        .as_ref()
        .context("minimization replay is missing")?;
    replay.validate(request)?;
    let config = replay
        .configuration
        .minimization
        .as_ref()
        .context("minimization configuration is missing")?;
    ensure!(
        receipt["schema"] == 1
            && receipt["scope"] == "native_minimize_host"
            && receipt["resource_id"] == request.resource_id
            && receipt["owner"] == serde_json::to_value(&request.owner)?
            && receipt["request_sha256"] == digest(request)?
            && receipt["jobs_empty"] == true
            && receipt["profiles_removed"] == true
            && receipt["scratch_removed"] == true
            && receipt["reducer"] == "fixture3-ddmin",
        "minimization receipt identity or cleanup differs"
    );
    let rows = receipt["trials"]
        .as_array()
        .context("minimization trial manifest missing")?;
    ensure!(
        !rows.is_empty()
            && rows.len() <= usize::from(config.max_attempts)
            && receipt["attempts"].as_u64() == Some(rows.len() as u64)
            && receipt["original_size"] == replay.input.size,
        "minimization attempts or original size differ"
    );
    let complete = receipt["complete"]
        .as_bool()
        .context("minimization completion missing")?;
    let cancelled = receipt["cancelled"]
        .as_bool()
        .context("minimization cancellation missing")?;
    ensure!(
        !complete
            || (!cancelled && receipt["stop_reason"].is_null() && receipt["failed"].is_null()),
        "interrupted or failed reduction cannot be complete"
    );
    let strength = if config.allow_signal_only {
        "signal_only"
    } else {
        "location_and_frames"
    };
    ensure!(
        receipt["equivalence_policy"] == strength
            && receipt["signature_strength"]
                == if receipt["crash_signature"].is_null() {
                    "unresolved"
                } else {
                    strength
                },
        "minimization equivalence policy or observed strength changed"
    );
    let mut best_verified = false;
    for (index, row) in rows.iter().enumerate() {
        let index = u16::try_from(index)?;
        let bytes = read(&format!("trial-{index}.json"))?;
        ensure!(
            row["trial"] == index
                && row["sha256"] == hex::encode(Sha256::digest(&bytes))
                && row["path"] == format!("native-results/{}/trial-{index}", request.resource_id)
                && row["input_path"]
                    == format!("native-results/{}/trial-{index}-input", request.resource_id),
            "minimization trial order, selector or hash differs"
        );
        let value: Value = serde_json::from_slice(&bytes)?;
        ensure!(
            value["trial"] == index
                && value["resource_id"] == request.resource_id
                && value["owner"] == serde_json::to_value(&request.owner)?
                && value["request_sha256"] == digest(request)?,
            "minimization trial belongs to another execution"
        );
        resource_process::output(request, &value["native"])?;
        ensure!(
            value["native"]["trial"] == index
                && value["native"]["scratch_nonce"]
                    == resource_replay::nonce(
                        &request.owner.identity.installation_id,
                        &request.resource_id,
                        index
                    )?,
            "minimization target persona differs"
        );
        let input = read(&format!("trial-{index}.input"))?;
        let hash = hex::encode(Sha256::digest(&input));
        let candidate: Reference = serde_json::from_value(value["input"].clone())?;
        ensure!(
            candidate.path == replay.input.path
                && candidate.size == input.len() as u64
                && candidate.size <= replay.input.size
                && !candidate.executable
                && candidate.sha256 == hash
                && candidate.id
                    == digest(&(
                        "native.minimize.candidate.v1",
                        &request.resource_id,
                        index,
                        &hash
                    ))?,
            "minimization candidate content or binding differs"
        );
        if index == 0 {
            ensure!(
                hash == replay.input.sha256 && candidate.size == replay.input.size,
                "minimization baseline differs from original input"
            );
        }
        if complete {
            ensure!(
                value["resolved"] == true,
                "unresolved trial cannot prove minimality"
            );
        }
        if value["crash_signature"] == receipt["crash_signature"]
            && !value["crash_signature"].is_null()
        {
            let exit = u32::try_from(
                value["native"]["exit_code"]
                    .as_u64()
                    .context("native exit missing")?,
            )?;
            ensure!(
                value["resolved"] == true
                    && value["crashed"] == true
                    && value["native"]["stop_reason"] == "exited",
                "accepted candidate did not produce a resolved crash"
            );
            if config.allow_signal_only {
                ensure!(
                    value["crash_signature"].as_str()
                        == resource_replay::signature(exit).as_deref(),
                    "weak crash signature differs from actual native exit"
                );
            } else {
                let observation = &value["observation"];
                ensure!(
                    observation["location_signature"] == value["crash_signature"]
                        && observation["exception_code"] == exit
                        && observation["input"] == value["input"]
                        && observation["request_sha256"] == digest(request)?
                        && observation["cleanup"]["jobs_empty"] == true
                        && observation["cleanup"]["profiles_removed"] == true
                        && observation["cleanup"]["scratch_removed"] == true,
                    "strong crash observation does not bind native execution and cleanup"
                );
            }
            best_verified |= receipt["artifact"]["sha256"] == hash
                && receipt["minimized_size"] == candidate.size;
        }
    }
    if receipt["failed"].is_null() {
        ensure!(best_verified, "minimized result was never verified");
        let minimized = read("minimized")?;
        ensure!(
            receipt["artifact"]["path"]
                == format!("native-results/{}/minimized", request.resource_id)
                && receipt["artifact"]["sha256"] == hex::encode(Sha256::digest(&minimized))
                && receipt["artifact"]["size"] == minimized.len()
                && receipt["minimized_size"] == minimized.len(),
            "minimized artifact differs"
        );
    } else {
        ensure!(
            receipt["artifact"].is_null() && receipt["minimized_size"].is_null(),
            "failed baseline published a minimized artifact"
        );
    }
    let mut result = receipt.clone();
    result["trials"] = json!(rows.len());
    result["trial_manifest"] = json!(format!("native-results/{}/receipt", request.resource_id));
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn library_reduction_preserves_order_and_only_the_original_crash_identity() -> Result<()> {
        let mut indices = Vec::new();
        let (best, attempts, complete, stopped) = reduce(
            b"xB\xffx\0y".to_vec(),
            128,
            |candidate, index| {
                indices.push(index);
                // B represents a different fault with the same exception code.
                Ok(Some(candidate.contains(&0xff) && candidate.contains(&0)))
            },
            || Ok(None),
        )?;
        assert_eq!(best, [0xff, 0]);
        assert_eq!(indices, (1..attempts).collect::<Vec<_>>());
        assert!(complete && stopped.is_none());
        Ok(())
    }
    #[test]
    fn empty_input_is_checked_even_when_no_single_deletion_is_interesting() -> Result<()> {
        let (best, attempts, complete, _) = reduce(
            b"AB".to_vec(),
            3,
            |candidate, _| Ok(Some(candidate.is_empty() || candidate == b"AB")),
            || Ok(None),
        )?;
        assert!(best.is_empty() && complete);
        assert_eq!(attempts, 2);
        Ok(())
    }
    #[test]
    fn baseline_consumes_the_attempt_budget_without_reexecution() -> Result<()> {
        let (best, attempts, complete, stopped) = reduce(
            b"A".to_vec(),
            1,
            |_, _| panic!("baseline was reexecuted"),
            || Ok(None),
        )?;
        assert_eq!(best, b"A");
        assert_eq!(attempts, 1);
        assert!(!complete && stopped.is_some());
        Ok(())
    }
    #[test]
    fn unresolved_oracle_never_executes_again_or_claims_minimality() -> Result<()> {
        let mut calls = 0;
        let (best, attempts, complete, stopped) = reduce(
            b"ABCDE".to_vec(),
            128,
            |_, _| {
                calls += 1;
                assert_eq!(calls, 1);
                Ok(None)
            },
            || Ok(None),
        )?;
        assert_eq!(best, b"ABCDE");
        assert_eq!(attempts, 2);
        assert!(!complete);
        assert_eq!(stopped.as_deref(), Some("candidate outcome unresolved"));
        Ok(())
    }
    #[test]
    fn cancellation_or_deadline_does_not_execute_a_candidate() -> Result<()> {
        for reason in ["cancelled", "deadline expired", "evidence budget exhausted"] {
            let (best, attempts, complete, stopped) = reduce(
                b"A".to_vec(),
                4096,
                |_, _| panic!("executed after stop"),
                || Ok(Some(reason)),
            )?;
            assert_eq!(best, b"A");
            assert_eq!(attempts, 1);
            assert!(!complete);
            assert_eq!(stopped.as_deref(), Some(reason));
        }
        Ok(())
    }
    #[test]
    fn unexpected_evaluation_failure_is_not_a_successful_reduction() {
        let error = reduce(
            b"A".to_vec(),
            4,
            |_, _| anyhow::bail!("evidence failed"),
            || Ok(None),
        )
        .unwrap_err();
        assert!(error.to_string().contains("evidence failed"));
    }
    #[test]
    fn all_trial_personas_are_distinct_beyond_the_old_u8_boundary() -> Result<()> {
        let mut identities = std::collections::BTreeSet::new();
        let (installation, resource) = ("a".repeat(32), "b".repeat(32));
        for index in [0, 15, 16, 255, 256, 4095] {
            assert!(identities.insert(resource_replay::nonce(&installation, &resource, index)?));
            assert!(identities.insert(target_nonce(&installation, &resource, index)?));
            assert!(identities.insert(debugger_nonce(&installation, &resource, index)?));
        }
        assert!(target_nonce(&installation, &resource, 4096).is_err());
        Ok(())
    }
}
