//! Fixed System child for one admitted debugger or interactive session. It owns the
//! contained identities and Jobs; model PIDs and tool paths are never authority.
use crate::{
    debugger_mcp::Client,
    deployment,
    inputs::digest,
    process_capture::CaptureWatch,
    processes::OwnedProcess,
    security::{self, AclPolicy},
    session_artifacts,
    session_commands::Session,
    session_request::Request as Target,
    session_target::Prepared,
    session_wire::{self, Channel, Reply, Request, reply::Outcome, request::Operation},
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    time::{Duration, Instant},
};

pub(crate) const SCHEMA: u32 = 2;
pub(crate) fn nonce(installation: &str, resource: &str) -> Result<String> {
    crate::processes::resource_job::name(installation, resource)?;
    Ok(digest(&("native.debugger.persona.v1", installation, resource))?[..48].into())
}
struct End {
    request: Request,
    result: Value,
}
type Controller = Channel<tokio::io::Stdin, tokio::io::Stdout>;

pub(crate) async fn watch(
    cancel: &Path,
    deadline: Instant,
    output: Option<&CaptureWatch>,
    diagnostics: &CaptureWatch,
    helper: &OwnedProcess,
) -> Result<()> {
    loop {
        ensure!(!helper.exited(), "owned MCP server exited");
        ensure!(!cancel.try_exists()?, "native debugger was cancelled");
        ensure!(
            !output.is_some_and(CaptureWatch::failed) && !diagnostics.failed(),
            "native debugger target exceeded its output budget or lost its streams"
        );
        ensure!(
            Instant::now() < deadline,
            "native debugger resource deadline expired"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
fn response(
    target: &Target,
    request: &Request,
    result: Value,
    state: &str,
    transcript: String,
) -> Result<Reply> {
    let encoded = serde_json::to_vec(&result)?;
    let result = if encoded.len() > 32768 {
        json!({"omitted":true,"bytes":encoded.len(),"sha256":hex::encode(Sha256::digest(&encoded)),"detail":"Full MCP results are retained in the transcript"})
    } else {
        result
    };
    let value = json!({"session_id":target.target.resource_id,"worker_session_id":target.target.resource_id,
        "native_owner":target.target.owner,"engine":"dbgeng","state":state,
        "mode":if target.read_only(){"dump"}else{"launch"},"read_only":target.read_only(),
        "transcript":transcript,"response":result,"production_execution":false});
    Ok(Reply {
        schema: session_wire::SCHEMA,
        resource: target.target.resource_id.clone(),
        sequence: request.sequence,
        request_sha256: digest(target)?,
        outcome: Some(Outcome::ResultJson(serde_json::to_vec(&value)?)),
    })
}

struct Serving<'a> {
    request: &'a Target,
    root: &'a Path,
    cancel: &'a Path,
    helper: &'a OwnedProcess,
    diagnostics: &'a CaptureWatch,
}
async fn serve(
    channel: &mut Controller,
    client: Client,
    target: &mut Prepared,
    context: Serving<'_>,
) -> Result<End> {
    let Serving {
        request,
        root,
        cancel,
        helper,
        diagnostics,
    } = context;
    let output = target.watch();
    let deadline = Instant::now() + Duration::from_secs(request.target.timeout_secs);
    let mut sequence = 0;
    // Only Open may use this unattached client. Later commands own a session.
    let mut unopened = Some(client);
    let mut session: Option<Session> = None;
    loop {
        ensure!(!helper.exited(), "owned MCP server exited");
        let next: Request = tokio::select! {
            next=channel.receive()=>next?.context("native controller channel closed")?,
            stopped=watch(cancel,deadline,output.as_ref(),diagnostics,helper)=>{stopped?;unreachable!()},
        };
        ensure!(
            next.schema == session_wire::SCHEMA
                && next.resource == request.target.resource_id
                && next.sequence == sequence + 1
                && next.sequence <= 4096,
            "native session request binding or sequence differs"
        );
        sequence = next.sequence;
        let operation = next
            .operation
            .clone()
            .context("native session operation missing")?;
        if matches!(operation, Operation::Close(_)) {
            ensure!(sequence > 1, "cannot close a session before Open");
            let owned = session
                .take()
                .context("native debugger session is absent")?;
            let result = tokio::select! {
                result=owned.close()=>result?,
                stopped=watch(cancel,deadline,output.as_ref(),diagnostics,helper)=>{stopped?;unreachable!()},
            };
            return Ok(End {
                request: next,
                result,
            });
        }
        let operation = async {
            match operation {
                Operation::Open(_) => {
                    ensure!(
                        sequence == 1,
                        "native debugger cannot reopen a live session"
                    );
                    let mut client = unopened
                        .take()
                        .context("native debugger was already opened")?;
                    let mut opened = target.open(&mut client).await?;
                    let id = opened["session_id"]
                        .as_str()
                        .context("MCP opener omitted its session handle")?
                        .to_owned();
                    let identity = target.identity()?;
                    let mut symbols =
                        json!({"session_id":id,"path":target.symbols(),"append":false});
                    if identity.has_symbols() {
                        // Fixed tool argument: no embedded filename becomes a command.
                        symbols["reload"] = json!("/f");
                    }
                    client.call("set_symbol_path", symbols).await?;
                    let modules = client
                        .structured("modules", json!({"session_id":id,"limit":2000}))
                        .await?;
                    let observed = identity.observed(&modules)?;
                    opened["identity"] = json!({"admitted":identity,"observed_module":observed,
                        "correspondence":if request.read_only(){"pe_pdb_and_dump_metadata"}else{"pe_and_pdb_metadata"},"loaded_pdb_bytes_verified":false});
                    session = Some(Session::new(client, id, request.read_only()));
                    Ok(opened)
                }
                Operation::Inspect(inspect) => {
                    let timeout = inspect.timeout_seconds;
                    inspect.validate()?;
                    tokio::time::timeout(
                        Duration::from_secs(timeout.into()),
                        session
                            .as_mut()
                            .context("native debugger is not open")?
                            .inspect(inspect),
                    )
                    .await
                    .context("native debugger inspection timed out")?
                }
                Operation::Control(control) => {
                    let timeout = control.timeout_seconds;
                    control.validate()?;
                    tokio::time::timeout(
                        Duration::from_secs(timeout.into()),
                        session
                            .as_mut()
                            .context("native debugger is not open")?
                            .control(control),
                    )
                    .await
                    .context("native debugger control outcome is unknown after timeout")?
                }
                Operation::Close(_) => unreachable!(),
                Operation::Write(_) | Operation::Profile(_) => {
                    anyhow::bail!("debugger sessions do not accept interactive operations")
                }
            }
        };
        let result = tokio::select! {
            result=operation=>result?,
            stopped=watch(cancel,deadline,output.as_ref(),diagnostics,helper)=>{stopped?;unreachable!()},
        };
        let transcript = session_artifacts::snapshot(
            root,
            &request.target.resource_id,
            sequence,
            &digest(request)?,
        )?;
        channel
            .send(&response(request, &next, result, "running", transcript)?)
            .await?;
    }
}

pub fn run(installation: &str, configuration: &Path, request_path: &Path) -> Result<()> {
    security::require_system()?;
    let host = crate::resource_host::open(installation, configuration)?;
    let private = AclPolicy::private_vm();
    private.verify(request_path, false)?;
    let request: Target = serde_json::from_slice(&deployment::read_bytes(request_path, 65536)?)?;
    request.validate()?;
    let id = &request.target.resource_id;
    let root = &host.root;
    let prefix = crate::session_request::prefix(request.kind())?;
    ensure!(
        root.join("metadata/resource-requests")
            .join(format!("{id}.{prefix}.json"))
            .canonicalize()?
            == request_path.canonicalize()?
            && request.target.owner.identity.installation_id == installation
            && request.target.owner.identity.package_sha256 == host.manifest.sha256(),
        "native debugger request slot or deployment differs"
    );
    crate::processes::resource_job::require_membership(installation, id)?;
    let cancel = root
        .join("metadata/resource-requests")
        .join(format!("{id}.cancel"));
    ensure!(
        !cancel.try_exists()?,
        "native debugger cancelled before preparation"
    );
    if request.kind() == crate::resource_journal::ResourceKind::Interactive {
        return crate::interactive_host::run(&host, &request, &cancel);
    }
    let debugger_nonce = nonce(installation, id)?;
    let mut backend = crate::session_backend::Backend::launch(
        &host,
        &request,
        &debugger_nonce,
        session_artifacts::create(root, id)?,
        |debugger, scratch| Prepared::prepare(&host, &request, debugger, scratch),
    )?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let mut channel = Channel::new(tokio::io::stdin(), tokio::io::stdout());
    let outcome = runtime.block_on(async {
        let client = backend.connect().await?;
        serve(
            &mut channel,
            client,
            &mut backend.target,
            Serving {
                request: &request,
                root,
                cancel: &cancel,
                helper: &backend.helper,
                diagnostics: backend.diagnostics.watch(),
            },
        )
        .await
    });
    let closed = backend.close();
    // Always shut down Tokio, even when cleanup or evidence publication fails.
    let sent = (|| -> Result<()> {
        let crate::session_backend::Closed {
            captured,
            diagnostics,
            identity,
        } = closed?;
        let result_directory = root.join("metadata/resource-results");
        private.publish(
            &result_directory.join(format!("{id}.debug-stderr")),
            &diagnostics,
        )?;
        let mut proof = json!({"schema":SCHEMA,"scope":"native_debugger_host","resource_id":id,
        "owner":request.target.owner,"request_sha256":digest(&request)?,"identity":identity,"outcome":if outcome.is_ok(){"closed"}else{"unknown"},
        "target_executed":captured.is_some(),"read_only":request.read_only(),
        "jobs_empty":true,"profiles_removed":true,"scratch_removed":true,"production_execution":false});
        if let Some(captured) = captured {
            private.publish(
                &result_directory.join(format!("{id}.stdout")),
                &captured.stdout,
            )?;
            private.publish(
                &result_directory.join(format!("{id}.stderr")),
                &captured.stderr,
            )?;
            proof["stdout_sha256"] = json!(hex::encode(Sha256::digest(&captured.stdout)));
            proof["stderr_sha256"] = json!(hex::encode(Sha256::digest(&captured.stderr)));
        }
        deployment::publish_json(
            &result_directory.join(format!("{id}.debug-result.json")),
            &proof,
            &private,
        )?;
        match outcome {
            Ok(end) => {
                let transcript = session_artifacts::snapshot(
                    root,
                    id,
                    end.request.sequence,
                    &digest(&request)?,
                )?;
                let reply = response(
                    &request,
                    &end.request,
                    json!({"release":end.result,"cleanup":proof}),
                    "closed",
                    transcript,
                )?;
                runtime.block_on(channel.send(&reply))
            }
            Err(error) => Err(error),
        }
    })();
    runtime.shutdown_timeout(Duration::from_secs(5));
    sent
}
