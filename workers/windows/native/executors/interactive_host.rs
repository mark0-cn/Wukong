//! Interactive business operations in the existing owned System session host.
use crate::{
    artifacts::inputs::digest,
    artifacts::session_artifacts::{self, Transcript},
    executors::interactive_io::Pipes,
    executors::resource_target::Prepared,
    executors::session_request::Request as Target,
    executors::session_wire::{self, Channel, Reply, Request, reply::Outcome, request::Operation},
    host::resource_host::Host,
    isolation::process_capture::PreparedIo,
    isolation::processes::{OwnedProcess, ResourceLimits},
    isolation::security::AclPolicy,
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{path::Path, time::Duration};

type Controller = Channel<tokio::io::Stdin, tokio::io::Stdout>;
struct End {
    request: Request,
    sent: bool,
    timed_out: bool,
    state: &'static str,
}

fn response(
    target: &Target,
    request: &Request,
    mut value: Value,
    state: &str,
    root: &Path,
) -> Result<Reply> {
    let id = &target.target.resource_id;
    let hash = digest(target)?;
    value["session_id"] = json!(id);
    value["worker_session_id"] = json!(id);
    value["native_owner"] = json!(target.target.owner);
    value["kind"] = json!("interactive");
    value["state"] = json!(state);
    value["isolation"] = json!("appcontainer");
    value["transcript"] = json!(session_artifacts::snapshot(
        root,
        id,
        request.sequence,
        &hash
    )?);
    let bytes = serde_json::to_vec(&value)?;
    ensure!(
        bytes.len() <= 60 * 1024,
        "interactive reply exceeds durable result bound"
    );
    Ok(Reply {
        schema: session_wire::SCHEMA,
        resource: id.clone(),
        sequence: request.sequence,
        request_sha256: hash,
        outcome: Some(Outcome::ResultJson(bytes)),
    })
}

async fn serve(
    channel: &mut Controller,
    target: &Target,
    process: &OwnedProcess,
    io: &mut Pipes,
    root: &Path,
) -> Result<End> {
    let mut sequence = 0;
    loop {
        let next: Request = tokio::select! {
            next = channel.receive() => next?.context("native controller channel closed")?,
            output = io.read(), if !io.output.eof => { output?; continue; },
        };
        ensure!(
            next.schema == session_wire::SCHEMA
                && next.resource == target.target.resource_id
                && next.sequence == sequence + 1
                && next.sequence <= 4096,
            "native session request binding or sequence differs"
        );
        sequence = next.sequence;
        let operation = next
            .operation
            .clone()
            .context("native session operation missing")?;
        let result = match operation {
            Operation::Open(_) => {
                ensure!(sequence == 1, "interactive session cannot reopen");
                io.output
                    .event(&json!({"operation":"open","pid":process.pid()}))?;
                json!({"ownership":"single_agent_serialized","pid":process.pid(),"input_mode":"stdin_pipe"})
            }
            Operation::Write(write) => {
                ensure!(sequence > 1, "interactive session is not open");
                write.validate()?;
                let sent = !process.exited();
                let timed_out = if sent { io.write(&write).await? } else { false };
                if process.exited() {
                    return Ok(End {
                        request: next,
                        sent,
                        timed_out,
                        state: "exited",
                    });
                }
                let mut value = io.output.snapshot(sent, timed_out);
                value["exit_code"] = Value::Null;
                value
            }
            Operation::Profile(_) => {
                ensure!(sequence > 1, "interactive session is not open");
                // Observation errors (including natural exit) must not terminate
                // an otherwise usable session or consume its pending stdout.
                let observed = match process.profile() {
                    Ok(value) => value,
                    Err(error) => json!({"status":"unavailable",
                        "error":format!("{error:#}").chars().take(512).collect::<String>()}),
                };
                io.output
                    .event(&json!({"operation":"profile","sequence":sequence,
                    "process":observed,"launch_identity":target.target.program}))?;
                json!({"process":crate::isolation::process_profile::preview(observed),
                    "launch_identity":target.target.program})
            }
            Operation::Close(_) => {
                ensure!(sequence > 1, "interactive session is not open");
                return Ok(End {
                    request: next,
                    sent: false,
                    timed_out: false,
                    state: "closed",
                });
            }
            _ => anyhow::bail!("unsupported interactive session operation"),
        };
        channel
            .send(&response(target, &next, result, "running", root)?)
            .await?;
    }
}

pub(crate) fn run(host: &Host, request: &Target, cancel: &Path) -> Result<()> {
    let root = &host.root;
    let id = &request.target.resource_id;
    let mut target = Prepared::open(host, &request.target, None)?;
    ensure!(
        target.input.is_none(),
        "interactive stdin cannot have a separate source"
    );
    let launched = PreparedIo::merged(request.target.output_limit)?.launch_pipes(|io| {
        target.launch(
            io,
            ResourceLimits::new(request.target.processes, request.target.memory_bytes)?,
        )
    })?;
    let process = launched.process;
    drop(launched.error);
    let mut io = Pipes::new(
        launched.input,
        launched.output,
        launched.limit,
        Transcript::new(session_artifacts::create(root, id)?),
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let mut channel = Channel::new(tokio::io::stdin(), tokio::io::stdout());
    let outcome = runtime.block_on(async {
        let watch = async {
            let deadline =
                tokio::time::Instant::now() + Duration::from_secs(request.target.timeout_secs);
            loop {
                ensure!(
                    !cancel.try_exists()?,
                    "native interactive session was cancelled"
                );
                ensure!(
                    tokio::time::Instant::now() < deadline,
                    "native interactive resource deadline expired"
                );
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        };
        tokio::select! {
            result = serve(&mut channel, request, &process, &mut io, root) => result,
            result = watch => { result?; unreachable!() },
        }
    });
    // These scopes retain Job authority until every blocking pipe peer is gone.
    let stopped = process.stop(Duration::from_secs(10));
    let pid = process.pid();
    let exit_code = process.exit_code();
    drop(process);
    let sent = (|| -> Result<()> {
        stopped?;
        runtime.block_on(io.drain())?;
        target.close()?;
        let private = AclPolicy::private_vm();
        private.publish(
            &root
                .join("metadata/resource-results")
                .join(format!("{id}.stdout")),
            io.output.bytes(),
        )?;
        let proof = json!({"schema":crate::executors::session_host::SCHEMA,"scope":request.scope(),
            "resource_id":id,"owner":request.target.owner,"request_sha256":digest(request)?,
            "program":request.target.program,"pid":pid,
            "outcome":if outcome.is_ok(){"closed"}else{"unknown"},"read_only":false,"target_executed":true,
            "jobs_empty":true,"profiles_removed":true,"scratch_removed":true,"isolation":"appcontainer",
            "stdout_sha256":hex::encode(Sha256::digest(io.output.bytes())),"exit_code":exit_code?});
        crate::artifacts::io::publish_json(
            &root
                .join("metadata/resource-results")
                .join(format!("{id}.interact-result.json")),
            &proof,
            &private,
        )?;
        match outcome {
            Ok(end) => {
                let mut value = io.output.snapshot(end.sent, end.timed_out);
                value["exit_code"] = proof["exit_code"].clone();
                value["response"] = json!({"cleanup":proof});
                io.output.event(
                    &json!({"operation":"end","state":end.state,"sequence":end.request.sequence}),
                )?;
                runtime.block_on(channel.send(&response(
                    request,
                    &end.request,
                    value,
                    end.state,
                    root,
                )?))
            }
            Err(error) => Err(error),
        }
    })();
    runtime.shutdown_timeout(Duration::from_secs(5));
    sent
}
