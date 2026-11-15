use crate::executors::resource_replay::{Configuration, Expected, InputMode, Kind, Replay};
use crate::executors::specification::default_output;
use crate::executors::specification::{Specification, default_cwd};
use crate::{
    artifacts::resource_input::Purpose,
    executors::parameters::start_parameters,
    executors::plan::{Plan, Response},
};
use anyhow::{Result, ensure};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
// Replay admission and reporting share process ownership and recovery.

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Parameters {
    pub(crate) target: String,
    pub(crate) input: String,
    pub(crate) args: Option<Vec<String>>,
    pub(crate) launch_spec: Option<String>,
    pub(crate) input_mode: Option<InputMode>,
    #[serde(default)]
    pub(crate) env: BTreeMap<String, String>,
    pub(crate) timeout_secs: Option<u64>,
    #[serde(default = "default_output")]
    pub(crate) output_limit: usize,
    pub(crate) trials: Option<u8>,
    pub(crate) max_attempts: Option<u16>,
    pub(crate) minimize_timeout_secs: Option<u64>,
    pub(crate) allow_signal_only: Option<bool>,
    pub(crate) expected: Option<Expected>,
    pub(crate) producer_agent_id: Option<String>,
    pub(crate) verifier_agent_id: Option<String>,
}
impl Parameters {
    pub(crate) fn plan(
        self,
        kind: Kind,
        request_id: &str,
    ) -> Result<(Specification, Configuration)> {
        ensure!(
            self.timeout_secs.is_none_or(|n| (1..=60).contains(&n))
                && (1024..=8 * 1024 * 1024).contains(&self.output_limit),
            "replay deadline/output bound differs"
        );
        let minimization = if kind == Kind::Minimize {
            ensure!(
                self.trials.is_none() && self.expected.is_none(),
                "minimization owns candidate trials and crash equivalence"
            );
            Some(crate::executors::resource_minimize::Configuration {
                max_attempts: self.max_attempts.unwrap_or(128),
                timeout_secs: self.minimize_timeout_secs.unwrap_or(300),
                allow_signal_only: self.allow_signal_only.unwrap_or(false),
            })
        } else {
            ensure!(
                self.max_attempts.is_none()
                    && self.minimize_timeout_secs.is_none()
                    && self.allow_signal_only.is_none(),
                "replay cannot accept minimization options"
            );
            None
        };
        let config = Configuration {
            kind,
            target: self.target.clone(),
            input: self.input.clone(),
            input_mode: self.input_mode,
            trials: minimization.as_ref().map_or_else(
                || {
                    u16::from(
                        self.trials
                            .unwrap_or(if kind == Kind::Verify { 3 } else { 1 }),
                    )
                },
                |minimization| minimization.max_attempts,
            ),
            minimization,
            expected: self.expected,
            call_id: request_id.into(),
            producer_agent_id: self.producer_agent_id,
            verifier_agent_id: self.verifier_agent_id,
        };
        config.validate()?;
        let spec = if self.launch_spec.is_some() {
            ensure!(
                self.args.is_none() && self.env.is_empty(),
                "LaunchSpec owns replay argv and environment"
            );
            Specification {
                argv: Vec::new(),
                launch_spec: self.launch_spec,
                input: Some(self.input),
                env: BTreeMap::new(),
                cwd: default_cwd(),
                timeout_secs: self.timeout_secs,
                output_limit: self.output_limit,
            }
        } else {
            let stdin = self.input_mode == Some(InputMode::Stdin);
            let mut argv = vec![self.target];
            for arg in self.args.unwrap_or_else(|| vec!["@@".into()]) {
                if arg == "@@" {
                    if !stdin {
                        argv.push(self.input.clone());
                    }
                } else {
                    argv.push(arg);
                }
            }
            ensure!(
                stdin || argv[1..].contains(&self.input),
                "argv replay must deliver its admitted input"
            );
            Specification {
                argv,
                launch_spec: None,
                input: stdin.then_some(self.input),
                env: self.env,
                cwd: default_cwd(),
                timeout_secs: Some(self.timeout_secs.unwrap_or(30)),
                output_limit: self.output_limit,
            }
        };
        spec.validate()?;
        Ok((spec, config))
    }
}

fn plan(params: &Map<String, Value>, kind: Kind) -> Result<Plan> {
    let (request_id, parameters): (_, Parameters) = start_parameters(params)?;
    let (spec, configuration) = parameters.plan(kind, request_id)?;
    let specification = json!({"native_process_schema":crate::executors::resource_process::SCHEMA,
            "process":spec,"replay":configuration});

    Ok(Plan {
        kind: crate::resources::models::ResourceKind::Process,
        request_id: request_id.into(),
        specification,
        response: Response::Job,
        prepare: Box::new(move |inputs, owner| {
            let mut request = spec.prepare(inputs, owner)?;
            request.replay = Some(Replay {
                input: inputs.reference(owner, &configuration.input, Purpose::Inspect)?,
                configuration,
            });
            request.validate()?;
            Ok(crate::executors::plan::Target::Process(request))
        }),
    })
}

pub(crate) fn replay(params: &Map<String, Value>) -> Result<Plan> {
    plan(params, Kind::Replay)
}

pub(crate) fn verify(params: &Map<String, Value>) -> Result<Plan> {
    plan(params, Kind::Verify)
}

pub(crate) fn minimize(params: &Map<String, Value>) -> Result<Plan> {
    plan(params, Kind::Minimize)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::state::tests::owner;
    use crate::resources::{journal::Reservation, models::ResourceKind};
    use crate::{
        artifacts::inputs::{Inputs, Owner},
        executors::{resource_process::Request, specification::Specification},
    };
    use base64::{Engine, engine::general_purpose::STANDARD};
    use sha2::{Digest, Sha256};
    use std::fs;

    fn parameters() -> Value {
        json!({"target":"files/known.exe","input":"files/input.bin","args":["--replay","@@"],
            "producer_agent_id":"producer","verifier_agent_id":"verifier","request_id":"verify-once"})
    }
    use crate::resources::state::tests::fixture;
    fn upload(inputs: &Inputs, name: &str, data: &[u8], executable: bool) -> Result<()> {
        let params =
            json!({"path":name,"data_base64":STANDARD.encode(data),"executable":executable});
        inputs
            .call(owner(), "file.put", params.as_object().unwrap())
            .map(|_| ())
    }
    fn prepared(inputs: &Inputs, params: Value) -> Result<(Specification, Configuration, Request)> {
        let (id, parameters): (_, Parameters) = start_parameters(params.as_object().unwrap())?;
        let (spec, config) = parameters.plan(Kind::Verify, id)?;
        let mut request = spec.prepare(inputs, &owner())?;
        request.replay = Some(Replay {
            configuration: config.clone(),
            input: inputs.reference(&owner(), &config.input, Purpose::Inspect)?,
        });
        request.validate()?;
        Ok((spec, config, request))
    }
    #[test]
    fn minimization_admission_validates_budgets_and_rejects_ignored_replay_fields() -> Result<()> {
        let (_root, inputs, mut runtime) = fixture()?;
        for (key, value) in [
            ("max_attempts", json!(0)),
            ("max_attempts", json!(4097)),
            ("minimize_timeout_secs", json!(0)),
            ("minimize_timeout_secs", json!(3601)),
            ("trials", json!(1)),
            ("expected", json!({"exit_code":0})),
        ] {
            let mut params = parameters();
            params[key] = value;
            assert!(
                crate::resources::state::tests::ready(crate::resources::state::tests::planned(
                    &mut runtime,
                    &inputs,
                    &owner(),
                    crate::executors::replay_plan::plan(
                        params.as_object().unwrap(),
                        Kind::Minimize
                    )
                ))
                .is_err(),
                "accepted {key}"
            );
            assert!(runtime.test_journal().idle()?);
        }
        let (_, params): (_, Parameters) = start_parameters(parameters().as_object().unwrap())?;
        let (_, settings) = params.plan(Kind::Minimize, "minimal-once")?;
        assert_eq!(settings.trials, 128);
        assert!(!settings.minimization.unwrap().allow_signal_only);
        let mut params = parameters();
        params["max_attempts"] = json!(4096);
        let (_, params): (_, Parameters) = start_parameters(params.as_object().unwrap())?;
        let (_, settings) = params.plan(Kind::Minimize, "maximal-once")?;
        assert_eq!(settings.trials, 4096);
        Ok(())
    }
    #[test]
    fn interrupted_replay_id_keeps_original_scope_and_never_admits_a_new_trial() -> Result<()> {
        let (root, inputs, mut runtime) = fixture()?;
        let params = parameters();
        let (id, parameters): (_, Parameters) = start_parameters(params.as_object().unwrap())?;
        let (spec, config) = parameters.plan(Kind::Verify, id)?;
        let intent = json!({"native_process_schema":crate::executors::resource_process::SCHEMA,"process":spec,"replay":config});
        let Reservation::Reserved {
            effect, resource, ..
        } = runtime
            .test_journal()
            .reserve_start(&owner(), id, ResourceKind::Process, &intent)?
        else {
            anyhow::bail!("missing reservation")
        };
        runtime.test_journal().begin(&effect)?;
        let successor = Owner {
            epoch: 2,
            owner_id: "successor".into(),
            ..owner()
        };
        let status =
            crate::resources::state::tests::ready(crate::resources::state::tests::planned(
                &mut runtime,
                &inputs,
                &successor,
                crate::executors::replay_plan::plan(params.as_object().unwrap(), Kind::Verify),
            ))?;
        assert_eq!(status["job_id"], resource);
        assert_eq!(status["state"], "lost");
        assert_eq!(status["kind"], "verify");
        assert_eq!(status["cleanup_pending"], true);
        let mut changed = params.clone();
        changed["trials"] = json!(2);
        assert!(
            crate::resources::state::tests::ready(crate::resources::state::tests::planned(
                &mut runtime,
                &inputs,
                &successor,
                crate::executors::replay_plan::plan(changed.as_object().unwrap(), Kind::Verify)
            ))
            .is_err()
        );
        assert!(
            crate::resources::state::tests::ready(crate::resources::state::tests::planned(
                &mut runtime,
                &inputs,
                &successor,
                crate::executors::replay_plan::plan(params.as_object().unwrap(), Kind::Replay)
            ))
            .is_err()
        );
        assert_eq!(fs::read_dir(root.path().join("native-inputs"))?.count(), 0);
        assert!(!runtime.test_journal().idle()?);
        Ok(())
    }
    #[test]
    fn replay_rejects_invalid_identity_delivery_and_bounds_before_reserving() -> Result<()> {
        let (root, inputs, mut runtime) = fixture()?;
        for (key, value) in [
            ("trials", json!(0)),
            ("trials", json!(17)),
            ("timeout_secs", json!(61)),
            ("output_limit", json!(1)),
            ("producer_agent_id", json!("verifier")),
            ("target", json!("../target.exe")),
            ("input_mode", json!("pty")),
            ("args", json!(["--unused-input"])),
            ("network", json!({})),
            ("expected", json!({"signal":"SIGSEGV"})),
            ("expected", json!({"exit_code":4294967296_u64})),
            ("call_id", json!("different")),
            ("launch_spec", json!("files/launch.json")),
        ] {
            let mut params = parameters();
            params[key] = value;
            assert!(
                crate::resources::state::tests::ready(crate::resources::state::tests::planned(
                    &mut runtime,
                    &inputs,
                    &owner(),
                    crate::executors::replay_plan::plan(params.as_object().unwrap(), Kind::Verify)
                ))
                .is_err(),
                "accepted {key}"
            );
            assert!(runtime.test_journal().idle()?);
        }
        assert_eq!(fs::read_dir(root.path().join("native-inputs"))?.count(), 0);
        Ok(())
    }
    #[test]
    fn replay_input_delivery_and_fresh_personas_bind_to_sealed_originals() -> Result<()> {
        let (_root, inputs, _runtime) = fixture()?;
        upload(
            &inputs,
            "known.exe",
            b"inert development input; never execute",
            true,
        )?;
        upload(&inputs, "input.bin", b"\0payload\xff", false)?;
        let (_, _, mut request) = prepared(&inputs, parameters())?;
        assert!(request.input.is_none());
        assert_eq!(request.arguments, ["--replay", "files/input.bin"]);
        assert_eq!(
            request.argument_files,
            [request.replay.as_ref().unwrap().input.clone()]
        );
        let mut params = parameters();
        params["input_mode"] = json!("stdin");
        let (_, _, stdin) = prepared(&inputs, params)?;
        assert!(stdin.argument_files.is_empty());
        assert_eq!(stdin.arguments, ["--replay"]);
        assert_eq!(
            stdin.input,
            Some(request.replay.as_ref().unwrap().input.clone())
        );
        let mut nonces = std::collections::BTreeSet::new();
        for index in 0..16 {
            assert!(nonces.insert(crate::executors::resource_replay::nonce(
                &owner().identity.installation_id,
                &request.resource_id,
                index
            )?));
        }
        assert!(!nonces.contains(&crate::executors::resource_process::nonce(
            &owner().identity.installation_id,
            &request.resource_id
        )?));
        assert!(
            crate::executors::session_request::Request {
                schema: crate::executors::session_host::SCHEMA,
                target: request.clone(),
                mode: crate::executors::session_request::Mode::Launch
            }
            .validate()
            .is_err()
        );
        request.replay.as_mut().unwrap().input.sha256 = "0".repeat(64);
        assert!(request.validate().is_err());
        Ok(())
    }
    #[test]
    fn replay_verdict_requires_all_original_trial_hashes_and_observed_outcomes() -> Result<()> {
        let (_root, inputs, _runtime) = fixture()?;
        upload(&inputs, "known.exe", b"inert fixture", true)?;
        upload(&inputs, "input.bin", b"data", false)?;
        let (_, _, request) = prepared(&inputs, parameters())?;
        let empty = hex::encode(Sha256::digest([]));
        let mut originals = Vec::new();
        let mut trials = Vec::new();
        for index in 0..3 {
            let value = json!({"schema":1,"scope":"native_process_host","resource_id":request.resource_id,
                "owner":request.owner,"program":request.program,"interpreter":null,"argument_files":request.argument_files,
                "launch":null,"input":null,"request_sha256":crate::artifacts::inputs::digest(&request)?,
                "pid":100+index,"execution_started":true,"exit_code":3221225477_u32,"stop_reason":"exited",
                "stdout_base64":"","stderr_base64":"","stdout_sha256":empty,"stderr_sha256":empty,
                "appcontainer_sid":format!("fixture-app-{index}"),"logon_sid":format!("fixture-logon-{index}"),
                "jobs_empty":true,"profiles_removed":true,"scratch_removed":true,"trial":index,
                "scratch_nonce":crate::executors::resource_replay::nonce(&owner().identity.installation_id, &request.resource_id,index)?});
            let bytes = serde_json::to_vec(&value)?;
            trials.push(json!({"trial":index,"sha256":hex::encode(Sha256::digest(&bytes))}));
            originals.push(bytes);
        }
        let receipt = json!({"schema":1,"scope":"native_replay_host","resource_id":request.resource_id,
            "owner":request.owner,"request_sha256":crate::artifacts::inputs::digest(&request)?,"trials":trials,"cancelled":false,
            "jobs_empty":true,"profiles_removed":true,"scratch_removed":true});
        let summary = crate::executors::resource_replay::summarize(&request, &receipt, |i| {
            Ok(originals[usize::from(i)].clone())
        })?;
        assert_eq!(summary["passed"], true);
        assert_eq!(summary["successes"], 3);
        assert!(
            summary["results"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["crash_signature"] == "exception:0xc0000005")
        );
        let mut missing = receipt.clone();
        missing["trials"].as_array_mut().unwrap().pop();
        assert!(
            crate::executors::resource_replay::summarize(&request, &missing, |i| Ok(originals
                [usize::from(i)]
            .clone()))
            .is_err()
        );
        missing["cancelled"] = json!(true);
        let interrupted = crate::executors::resource_replay::summarize(&request, &missing, |i| {
            Ok(originals[usize::from(i)].clone())
        })?;
        assert_eq!(interrupted["passed"], false);
        assert_eq!(interrupted["attempts"], 2);
        let mut tampered = originals.clone();
        tampered[1].push(b' ');
        assert!(
            crate::executors::resource_replay::summarize(&request, &receipt, |i| Ok(tampered
                [usize::from(i)]
            .clone()))
            .is_err()
        );
        let mut noncrash: Value = serde_json::from_slice(&originals[1])?;
        noncrash["exit_code"] = json!(0);
        originals[1] = serde_json::to_vec(&noncrash)?;
        let mut observed = receipt;
        observed["trials"][1]["sha256"] = json!(hex::encode(Sha256::digest(&originals[1])));
        let negative = crate::executors::resource_replay::summarize(&request, &observed, |i| {
            Ok(originals[usize::from(i)].clone())
        })?;
        assert_eq!(negative["passed"], false);
        assert_eq!(negative["successes"], 2);
        assert_eq!(negative["observed_stage"], "behavior");

        // Binary controls expand up to six bytes each in JSON. All 16 trials
        // must still fit the status budget while originals remain unmodified.
        let mut bounded = request.clone();
        let config = &mut bounded.replay.as_mut().unwrap().configuration;
        config.trials = 16;
        config.expected = Some(serde_json::from_value(json!({
            "stdout_contains":"\u{1}".repeat(1300)
        }))?);
        bounded.validate()?;
        let data = vec![1u8; 256];
        let mut expanded = Vec::new();
        let mut references = Vec::new();
        for index in 0..16 {
            let mut value: Value = serde_json::from_slice(&originals[0])?;
            value["request_sha256"] = json!(crate::artifacts::inputs::digest(&bounded)?);
            value["trial"] = json!(index);
            value["scratch_nonce"] = json!(crate::executors::resource_replay::nonce(
                &owner().identity.installation_id,
                &bounded.resource_id,
                index
            )?);
            for stream in ["stdout", "stderr"] {
                value[format!("{stream}_base64")] = json!(STANDARD.encode(&data));
                value[format!("{stream}_sha256")] = json!(hex::encode(Sha256::digest(&data)));
            }
            let bytes = serde_json::to_vec(&value)?;
            references.push(json!({"trial":index,"sha256":hex::encode(Sha256::digest(&bytes))}));
            expanded.push(bytes);
        }
        observed["request_sha256"] = json!(crate::artifacts::inputs::digest(&bounded)?);
        observed["trials"] = json!(references);
        let summary = crate::executors::resource_replay::summarize(&bounded, &observed, |i| {
            Ok(expanded[usize::from(i)].clone())
        })?;
        assert_eq!(summary["attempts"], 16);
        assert!(serde_json::to_vec(&summary)?.len() <= 60 * 1024);
        bounded.replay.as_mut().unwrap().configuration.expected = Some(serde_json::from_value(
            json!({"stdout_contains":"\u{1}".repeat(4096)}),
        )?);
        assert!(bounded.validate().is_err());
        Ok(())
    }

    #[test]
    fn minimization_summary_requires_a_verified_original_candidate_and_unchanged_output()
    -> Result<()> {
        let (_root, inputs, _runtime) = fixture()?;
        upload(&inputs, "known.exe", b"inert fixture", true)?;
        upload(&inputs, "input.bin", b"A", false)?;
        let (_, _, mut request) = prepared(&inputs, parameters())?;
        let replay = request.replay.as_mut().unwrap();
        replay.configuration.kind = Kind::Minimize;
        replay.configuration.trials = 1;
        replay.configuration.minimization =
            Some(crate::executors::resource_minimize::Configuration {
                max_attempts: 1,
                timeout_secs: 30,
                allow_signal_only: true,
            });
        request.validate()?;
        let input = request.replay.as_ref().unwrap().input.clone();
        let empty = hex::encode(Sha256::digest([]));
        let hash = hex::encode(Sha256::digest(b"A"));
        let candidate = crate::artifacts::resource_input::Reference {
            id: crate::artifacts::inputs::digest(&(
                "native.minimize.candidate.v1",
                &request.resource_id,
                0u16,
                &hash,
            ))?,
            ..input
        };
        let native = json!({"schema":1,"scope":"native_process_host","resource_id":request.resource_id,
            "owner":request.owner,"program":request.program,"interpreter":null,"argument_files":request.argument_files,
            "launch":null,"input":null,"request_sha256":crate::artifacts::inputs::digest(&request)?,
            "pid":100,"execution_started":true,"exit_code":3221225477_u32,"stop_reason":"exited",
            "stdout_base64":"","stderr_base64":"","stdout_sha256":empty,"stderr_sha256":empty,
            "jobs_empty":true,"profiles_removed":true,"scratch_removed":true,"trial":0,
            "scratch_nonce":crate::executors::resource_replay::nonce(&owner().identity.installation_id,&request.resource_id,0)?});
        let execution = json!({"schema":1,"scope":"native_minimize_execution","resource_id":request.resource_id,
            "owner":request.owner,"request_sha256":crate::artifacts::inputs::digest(&request)?,"trial":0,"input":candidate,"native":native});
        let native_bytes = serde_json::to_vec(&execution)?;
        let trial = json!({"schema":1,"resource_id":request.resource_id,"owner":request.owner,
            "request_sha256":crate::artifacts::inputs::digest(&request)?,"trial":0,"input":candidate,"native":native,
            "native_record":{"path":format!("native-results/{}/trial-0-native",request.resource_id),
                "size":native_bytes.len(),"sha256":hex::encode(Sha256::digest(&native_bytes))},
            "resolved":true,"crashed":true,"crash_signature":"exception:0xc0000005"});
        let bytes = serde_json::to_vec(&trial)?;
        let receipt = json!({"schema":1,"scope":"native_minimize_host","resource_id":request.resource_id,
            "owner":request.owner,"request_sha256":crate::artifacts::inputs::digest(&request)?,
            "trials":[{"trial":0,"sha256":hex::encode(Sha256::digest(&bytes)),
                "path":format!("native-results/{}/trial-0",request.resource_id),
                "input_path":format!("native-results/{}/trial-0-input",request.resource_id)}],
            "attempts":1,"original_size":1,"minimized_size":1,"artifact":{
                "path":format!("native-results/{}/minimized",request.resource_id),"sha256":hash,"size":1},
            "complete":false,"stop_reason":"attempt budget exhausted","cancelled":false,"failed":null,
            "crash_signature":"exception:0xc0000005","signature_strength":"signal_only","equivalence_policy":"signal_only",
            "reducer":"fixture3-ddmin","jobs_empty":true,"profiles_removed":true,"scratch_removed":true});
        let read = |suffix: &str| -> Result<Vec<u8>> {
            Ok(match suffix {
                "trial-0.json" => bytes.clone(),
                "trial-0.native.json" => native_bytes.clone(),
                "trial-0.input" | "minimized" => b"A".to_vec(),
                _ => anyhow::bail!("unexpected artifact"),
            })
        };
        let summary = crate::executors::resource_minimize::summarize(&request, &receipt, read)?;
        assert_eq!(summary["complete"], false);
        for (field, changed) in [
            ("complete", json!(true)),
            ("attempts", json!(2)),
            ("original_size", json!(2)),
            ("signature_strength", json!("location_and_frames")),
            ("trials", json!([])),
            ("crash_signature", json!("exception:0xc000001d")),
        ] {
            let mut changed_receipt = receipt.clone();
            changed_receipt[field] = changed;
            assert!(
                crate::executors::resource_minimize::summarize(&request, &changed_receipt, read)
                    .is_err(),
                "accepted {field}"
            );
        }
        for corrupt in [
            "trial-0.json",
            "trial-0.native.json",
            "trial-0.input",
            "minimized",
        ] {
            assert!(
                crate::executors::resource_minimize::summarize(&request, &receipt, |suffix| {
                    if suffix == corrupt {
                        Ok(b"changed".to_vec())
                    } else {
                        read(suffix)
                    }
                })
                .is_err(),
                "accepted changed {corrupt}"
            );
        }
        Ok(())
    }
}
