use crate::{
    executors::session_runtime,
    executors::session_wire,
    resources::journal::{Outcome, Reservation, Ticket},
};
use anyhow::{Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Deserialize;
use serde_json::Value;
// Journal admission and completion for an owned persistent session. No pipe wait
// holds the resource/input locks; upstream owns execution scheduling.

pub(crate) struct Pending {
    pub(super) resource: String,
    pub(super) ticket: Option<Ticket>,
    pub(super) close: bool,
    pub(super) exchange: session_runtime::Pending,
}
pub(crate) struct Completed {
    pub(super) resource: String,
    pub(super) ticket: Option<Ticket>,
    pub(super) close: bool,
    pub(super) exchange: session_runtime::Completed,
}
impl Pending {
    pub(crate) async fn exchange(self) -> Completed {
        Completed {
            resource: self.resource,
            ticket: self.ticket,
            close: self.close,
            exchange: self.exchange.exchange().await,
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct InteractiveWrite {
    pub(super) data_base64: String,
    #[serde(default = "read_timeout")]
    pub(super) read_timeout_secs: u32,
}
fn read_timeout() -> u32 {
    10
}
impl InteractiveWrite {
    pub(super) fn decode(self) -> Result<session_wire::Write> {
        ensure!(
            self.data_base64.len() <= 1_398_104,
            "interactive input encoding exceeds 1 MiB"
        );
        let request = session_wire::Write {
            data: STANDARD.decode(self.data_base64)?,
            read_timeout_seconds: self.read_timeout_secs,
        };
        request.validate()?;
        Ok(request)
    }
}
pub(super) fn cached(reservation: Reservation) -> Result<Value> {
    match reservation {
        Reservation::Completed {
            outcome: Outcome::Result { value },
            ..
        } => Ok(value),
        Reservation::Completed { .. } => {
            anyhow::bail!("native session outcome is unknown after cleanup; it cannot be replayed")
        }
        _ => anyhow::bail!(
            "native session request is pending or uncertain; reconcile the original resource"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::state::*;
    use crate::{artifacts::inputs::Inputs, executors::session_plan::Start};
    #[test]
    fn completed_binary_write_retries_return_original_result_without_a_live_host() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let owner = owner();
        let policy = AclPolicy::current_user()?;
        policy.apply(directory.path(), true)?;
        let journal = Journal::with_policy(directory.path(), owner.identity.clone(), policy)?;
        let mut runtime = crate::resources::state::tests::state(journal, directory.path());
        let specification = json!({"native_interactive_schema":crate::executors::session_host::SCHEMA,"process":{"argv":["files/target.exe"],"timeout_secs":3600,"env":{},"cwd":".","output_limit":1048576}});
        let Reservation::Reserved {
            effect, resource, ..
        } = runtime.journal.reserve_start(
            &owner,
            "start",
            ResourceKind::Interactive,
            &specification,
        )?
        else {
            anyhow::bail!("start not reserved")
        };
        let ticket = runtime.journal.begin(&effect)?;
        runtime.journal.complete(
            &ticket,
            &Outcome::Result {
                value: json!({"state":"running"}),
            },
        )?;
        let data = vec![255; 1024 * 1024];
        let intent = json!({"method":"interact.write","params":{"bytes":data.len(),"sha256":hex::encode(Sha256::digest(&data)),"read_timeout_secs":10}});
        let Reservation::Reserved { effect, .. } = runtime.journal.reserve_mutation(
            &owner,
            "write",
            &resource,
            MutationKind::InteractiveWrite,
            &intent,
        )?
        else {
            anyhow::bail!("write not reserved")
        };
        let ticket = runtime.journal.begin(&effect)?;
        let expected = json!({"state":"exited","session_id":resource,"captured":"original"});
        runtime.journal.complete(
            &ticket,
            &Outcome::Result {
                value: expected.clone(),
            },
        )?;
        runtime.journal.start_closing(&resource)?;
        runtime.journal.close(
            &resource,
            Cleanup {
                jobs_empty: true,
                profiles_removed: true,
                scratch_removed: true,
            },
        )?;
        let mut params = json!({"session_id":resource,"request_id":"write","data_base64":STANDARD.encode(&data)});
        let Dispatch::Ready(value) =
            runtime.prepare_rpc(&owner, "interact.write", params.as_object().unwrap())?
        else {
            anyhow::bail!("completed write was executed")
        };
        assert_eq!(value, expected);
        params["data_base64"] = json!(STANDARD.encode(b"different"));
        assert!(
            runtime
                .prepare_rpc(&owner, "interact.write", params.as_object().unwrap())
                .is_err()
        );
        params["data_base64"] = json!(STANDARD.encode(&data));
        params["read_timeout_secs"] = json!(11);
        assert!(
            runtime
                .prepare_rpc(&owner, "interact.write", params.as_object().unwrap())
                .is_err()
        );
        params["read_timeout_secs"] = json!(301);
        assert!(
            runtime
                .prepare_rpc(&owner, "interact.write", params.as_object().unwrap())
                .is_err()
        );
        assert!(runtime.journal.idle()?);
        let inputs = Inputs::open(directory.path(), AclPolicy::current_user()?)?;
        let params = json!({"request_id":"start","argv":["files/target.exe"]});
        let Dispatch::Ready(original) = crate::resources::state::tests::planned(
            &mut runtime,
            &inputs,
            &owner,
            crate::executors::session_plan::interactive(params.as_object().unwrap()),
        )?
        else {
            anyhow::bail!("closed start attempted new work")
        };
        assert_eq!(original, json!({"state":"running"}));
        assert!(runtime.journal.idle()?);
        Ok(())
    }
    fn owner() -> Owner {
        Owner {
            identity: crate::rpc::worker::Identity {
                protocol: 3,
                installation_id: "a".repeat(32),
                boot_id: format!("windows:{}:1", "b".repeat(32)),
                package_sha256: "c".repeat(64),
            },
            run_id: "run".into(),
            owner_id: "controller".into(),
            epoch: 1,
        }
    }
    #[test]
    fn debugger_start_reconciles_prior_intent_before_resolving_new_inputs() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let owner = owner();
        let policy = AclPolicy::current_user()?;
        policy.apply(directory.path(), true)?;
        let journal =
            Journal::with_policy(directory.path(), owner.identity.clone(), policy.clone())?;
        let inputs = Inputs::open(directory.path(), policy)?;
        let mut runtime = crate::resources::state::tests::state(journal, directory.path());
        for params in [
            json!({"request_id":"invalid","mode":"launch","args":["files/not-a-target.exe"]}),
            json!({"request_id":"invalid","mode":"dump","target":"files/image.exe","dump":"files/image.dmp"}),
            json!({"request_id":"invalid","mode":"launch","engine":"gdb_mi","target":"files/image.exe"}),
            json!({"request_id":"invalid","mode":"launch","launch_spec":"files/launch.json","target":"files/image.exe"}),
        ] {
            assert!(
                crate::resources::state::tests::planned(
                    &mut runtime,
                    &inputs,
                    &owner,
                    crate::executors::session_plan::debugger(params.as_object().unwrap())
                )
                .is_err()
            );
            assert!(runtime.journal.idle()?);
        }
        let params = json!({"request_id":"stable","mode":"launch","target":"files/image.exe"});
        let start: Start = serde_json::from_value(clean(params.as_object().unwrap()).into())?;
        let process: Specification =
            serde_json::from_value(json!({"argv":["files/image.exe"],"timeout_secs":3600}))?;
        let spec = json!({"native_debugger_schema":crate::executors::session_host::SCHEMA,"debugger":start,"process":process});
        let Reservation::Reserved {
            effect, resource, ..
        } = runtime
            .journal
            .reserve_start(&owner, "stable", ResourceKind::Debugger, &spec)?
        else {
            anyhow::bail!("reservation absent")
        };
        let _ticket = runtime.journal.begin(&effect)?;
        let mut successor = owner.clone();
        successor.owner_id = "successor".into();
        successor.epoch = 2;
        let error = match crate::resources::state::tests::planned(
            &mut runtime,
            &inputs,
            &successor,
            crate::executors::session_plan::debugger(params.as_object().unwrap()),
        ) {
            Err(error) => error,
            Ok(_) => anyhow::bail!("replayed interrupted start"),
        };
        assert!(error.to_string().contains("pending or uncertain"));
        assert!(!runtime.journal.idle()?);
        assert!(runtime.journal.begin(&effect).is_err());
        runtime.journal.start_closing(&resource)?;
        runtime.journal.close(
            &resource,
            Cleanup {
                jobs_empty: true,
                profiles_removed: true,
                scratch_removed: true,
            },
        )?;
        assert!(runtime.journal.idle()?);
        let error = match crate::resources::state::tests::planned(
            &mut runtime,
            &inputs,
            &successor,
            crate::executors::session_plan::debugger(params.as_object().unwrap()),
        ) {
            Err(error) => error,
            Ok(_) => anyhow::bail!("replayed closed start"),
        };
        assert!(error.to_string().contains("unknown after cleanup"));
        let mut changed = params;
        changed["target"] = json!("files/changed.exe");
        let error = match crate::resources::state::tests::planned(
            &mut runtime,
            &inputs,
            &successor,
            crate::executors::session_plan::debugger(changed.as_object().unwrap()),
        ) {
            Err(error) => error,
            Ok(_) => anyhow::bail!("changed original intent"),
        };
        assert!(!error.to_string().contains("native input"));
        assert!(runtime.journal.idle()?);
        Ok(())
    }
}
