//! Replay policy and evidence reuse the native process executor and cleanup.
//! Each bounded trial gets a distinct persona; no target runs in this host.
use crate::{
    deployment,
    inputs::digest,
    resource_host::Host,
    resource_input::Reference,
    resource_journal::bounded_identifier,
    resource_process::{self, Request},
    security::AclPolicy,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Replay,
    Verify,
    Minimize,
}
impl Kind {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Replay => "replay",
            Self::Verify => "verify",
            Self::Minimize => "minimize",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InputMode {
    Argv,
    Stdin,
}
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Expected {
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_code: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stdout_contains: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stderr_contains: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    pub kind: Kind,
    pub target: String,
    pub input: String,
    pub input_mode: Option<InputMode>,
    pub trials: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimization: Option<crate::resource_minimize::Configuration>,
    pub expected: Option<Expected>,
    pub call_id: String,
    pub producer_agent_id: Option<String>,
    pub verifier_agent_id: Option<String>,
}
impl Configuration {
    pub(crate) fn validate(&self) -> Result<()> {
        crate::resource_launch::logical(&self.target)?;
        crate::resource_launch::logical(&self.input)?;
        if let Some(minimization) = &self.minimization {
            minimization.validate()?;
            ensure!(
                self.kind == Kind::Minimize
                    && self.trials == minimization.max_attempts
                    && self.expected.is_none(),
                "minimization count or equivalence policy differs"
            );
        } else {
            ensure!(
                self.kind != Kind::Minimize && (1..=16).contains(&self.trials),
                "replay requires 1 through 16 trials"
            );
        }
        bounded_identifier(&self.call_id, 128)?;
        for id in [&self.producer_agent_id, &self.verifier_agent_id]
            .into_iter()
            .flatten()
        {
            bounded_identifier(id, 128)?;
        }
        if self.kind == Kind::Verify {
            ensure!(
                self.producer_agent_id.is_some()
                    && self.verifier_agent_id.is_some()
                    && self.producer_agent_id != self.verifier_agent_id,
                "verification requires distinct producer and verifier identities"
            );
        }
        if let Some(expected) = &self.expected {
            ensure!(
                serde_json::to_vec(expected)?.len() <= 8192,
                "expected output exceeds the 8-KiB encoded summary allowance"
            );
            ensure!(
                expected
                    .exit_code
                    .is_none_or(|code| (i32::MIN as i64..=u32::MAX as i64).contains(&code)),
                "Windows expected exit code is outside its 32-bit range"
            );
            for text in [&expected.stdout_contains, &expected.stderr_contains]
                .into_iter()
                .flatten()
            {
                ensure!(
                    text.len() <= 4096 && !text.contains('\0'),
                    "expected output exceeds 4 KiB or contains NUL"
                );
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Replay {
    pub configuration: Configuration,
    pub input: Reference,
}
impl Replay {
    pub(crate) fn mode(&self, request: &Request) -> Result<InputMode> {
        let mode = if let Some(launch) = &request.launch {
            match launch.input_mode {
                crate::resource_launch::InputMode::ArgvFile => InputMode::Argv,
                crate::resource_launch::InputMode::StdinPipe => InputMode::Stdin,
            }
        } else {
            self.configuration.input_mode.unwrap_or(InputMode::Argv)
        };
        ensure!(
            self.configuration
                .input_mode
                .is_none_or(|configured| configured == mode),
            "replay input mode differs from its LaunchSpec"
        );
        Ok(mode)
    }
    pub(crate) fn validate(&self, request: &Request) -> Result<()> {
        self.configuration.validate()?;
        ensure!(
            request.interpreter.is_none()
                && request.timeout_secs <= 60
                && request.program.path == self.configuration.target
                && self.input.path == self.configuration.input
                && !self.input.executable
                && self.input.size <= 32 * 1024 * 1024,
            "replay must bind a native image, sealed data input and at most 60 seconds per trial"
        );
        if request.launch.is_some() || self.mode(request)? == InputMode::Stdin {
            ensure!(
                request.input.as_ref() == Some(&self.input),
                "replay input differs from process input"
            );
        } else {
            ensure!(
                request.input.is_none() && request.argument_files.contains(&self.input),
                "argv replay requires its sealed argument-file input"
            );
        }
        self.mode(request)?;
        Ok(())
    }
}
pub(crate) fn nonce(installation: &str, resource: &str, trial: u16) -> Result<String> {
    resource_process::nonce(installation, resource)?;
    ensure!(trial < 4096, "invalid replay trial index");
    Ok(digest(&("native.replay.persona.v1", installation, resource, trial))?[..48].to_owned())
}

pub(crate) fn execute(host: &Host, request: &Request, cancel: &Path) -> Result<Value> {
    let replay = request.replay.as_ref().context("missing replay plan")?;
    if replay.configuration.minimization.is_some() {
        return crate::resource_minimize::execute(host, request, cancel);
    }
    let mut trials = Vec::new();
    let policy = AclPolicy::private_vm();
    let mut cancelled = false;
    for index in 0..replay.configuration.trials {
        if cancel.try_exists()? {
            cancelled = true;
            break;
        }
        let value = resource_process::execute_one(host, request, cancel, Some(index))?;
        let path = host
            .root
            .join("metadata/resource-results")
            .join(format!("{}.trial-{index}.json", request.resource_id));
        deployment::publish_json(&path, &value, &policy)?;
        let bytes = deployment::read_bytes(&path, 12 * 1024 * 1024)?;
        trials.push(json!({"trial":index,"sha256":hex::encode(Sha256::digest(&bytes))}));
        if value["stop_reason"] == "cancelled" {
            cancelled = true;
            break;
        }
        // execute_one returns only after its target Job, profile and scratch
        // are closed. The next trial cannot overlap its predecessor.
    }
    cancelled |= cancel.try_exists()?;
    Ok(
        json!({"schema":1,"scope":"native_replay_host","resource_id":request.resource_id,
        "owner":request.owner,"request_sha256":digest(request)?,"trials":trials,"cancelled":cancelled,
        "jobs_empty":true,"profiles_removed":true,"scratch_removed":true,"production_execution":false}),
    )
}

pub(crate) fn signature(exit: u32) -> Option<String> {
    matches!(
        exit,
        0xc0000005 | 0xc000001d | 0xc0000094 | 0xc00000fd | 0xc0000409
    )
    .then(|| format!("exception:0x{exit:x}"))
}
fn matches(expected: Option<&Expected>, exit: u32, stdout: &[u8], stderr: &[u8]) -> bool {
    let Some(expected) = expected.filter(|e| **e != Expected::default()) else {
        return signature(exit).is_some();
    };
    expected
        .exit_code
        .is_none_or(|code| code == i64::from(exit) || code == i64::from(exit as i32))
        && expected
            .stdout_contains
            .as_ref()
            .is_none_or(|text| String::from_utf8_lossy(stdout).contains(text))
        && expected
            .stderr_contains
            .as_ref()
            .is_none_or(|text| String::from_utf8_lossy(stderr).contains(text))
}
pub(crate) fn summarize(
    request: &Request,
    receipt: &Value,
    mut read: impl FnMut(u16) -> Result<Vec<u8>>,
) -> Result<Value> {
    let replay = request.replay.as_ref().context("missing replay plan")?;
    replay.validate(request)?;
    ensure!(
        receipt["schema"] == 1
            && receipt["scope"] == "native_replay_host"
            && receipt["resource_id"] == request.resource_id
            && receipt["owner"] == serde_json::to_value(&request.owner)?
            && receipt["request_sha256"] == digest(request)?
            && receipt["jobs_empty"] == true
            && receipt["profiles_removed"] == true
            && receipt["scratch_removed"] == true,
        "replay receipt does not bind original execution and cleanup"
    );
    let trials = receipt["trials"]
        .as_array()
        .context("missing replay trial receipts")?;
    let cancelled = receipt["cancelled"]
        .as_bool()
        .context("missing replay cancellation state")?;
    ensure!(
        trials.len() <= usize::from(replay.configuration.trials)
            && (cancelled || trials.len() == usize::from(replay.configuration.trials)),
        "replay trial count differs"
    );
    let mut rows = Vec::new();
    let mut successes = 0;
    for (index, trial) in trials.iter().enumerate() {
        let index = u16::try_from(index)?;
        ensure!(trial["trial"] == index, "replay trial order differs");
        let bytes = read(index)?;
        ensure!(
            bytes.len() <= 12 * 1024 * 1024
                && trial["sha256"] == hex::encode(Sha256::digest(&bytes)),
            "replay trial receipt hash differs"
        );
        let result: Value = serde_json::from_slice(&bytes)?;
        let streams = resource_process::output(request, &result)?;
        let nonce = nonce(
            &request.owner.identity.installation_id,
            &request.resource_id,
            index,
        )?;
        ensure!(
            result["trial"] == index && result["scratch_nonce"] == nonce,
            "trial persona identity differs"
        );
        let exit = u32::try_from(
            result["exit_code"]
                .as_u64()
                .context("trial exit code missing")?,
        )?;
        let completed = result["stop_reason"] == "exited" && result["execution_started"] == true;
        let crash = completed.then(|| signature(exit)).flatten();
        let success = completed
            && matches(
                replay.configuration.expected.as_ref(),
                exit,
                &streams[0],
                &streams[1],
            );
        successes += usize::from(success);
        let tail = |data: &[u8]| {
            String::from_utf8_lossy(&data[data.len().saturating_sub(128)..]).into_owned()
        };
        rows.push(json!({"trial":index,"state":if completed {"completed"} else {"incomplete"},
            "pid":result["pid"],"exit_code":exit,"stop_reason":result["stop_reason"],
            "execution_started":result["execution_started"],"input_sha256":replay.input.sha256,
            "input_mode":replay.mode(request)?,"target_sha256":request.program.sha256,
            "crash_signature":crash,"success":success,"stdout":tail(&streams[0]),"stderr":tail(&streams[1]),
            "stdout_sha256":result["stdout_sha256"],"stderr_sha256":result["stderr_sha256"],
            "appcontainer_sid":result["appcontainer_sid"],"logon_sid":result["logon_sid"],"scratch_nonce":nonce,
            "receipt_path":format!("native-results/{}/trial-{index}",request.resource_id),"receipt_sha256":trial["sha256"],
            "metadata_protected":true,"jobs_empty":true,"profiles_removed":true,"scratch_removed":true}));
    }
    let config = &replay.configuration;
    let value = json!({"trials":rows.len(),"attempts":rows.len(),"requested_trials":config.trials,
        "successes":successes,"passed":!cancelled && !rows.is_empty() && successes == rows.len() && rows.len() == usize::from(config.trials),
        "observed_stage":if !rows.is_empty() && rows.iter().all(|row| row["crash_signature"].is_string()) {"crash"} else {"behavior"},
        "results":rows,"producer_agent_id":config.producer_agent_id,"verifier_agent_id":config.verifier_agent_id,
        "call_id":config.call_id,"target":request.program,"input":replay.input,"expected":config.expected,
        "fresh_working_directory_per_trial":true,"environment":{"platform":"windows","worker":request.owner.identity},
        "jobs_empty":true,"profiles_removed":true,"scratch_removed":true});
    ensure!(
        serde_json::to_vec(&value)?.len() <= 60 * 1024,
        "replay summary exceeds native status bound"
    );
    Ok(value)
}
