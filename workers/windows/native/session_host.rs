//! Fixed System child for one admitted live debugger. It owns both contained
//! identities and their Jobs; neither a model PID nor a tool path is accepted.
use crate::{
    accounts::Role,
    debugger_mcp::Client,
    deployment,
    inputs::digest,
    process_capture::{CaptureWatch, CapturedProcess, CapturedStream, PreparedIo},
    processes::{OwnedProcess, ResourceLimits, Spawn},
    resource_process::{Request as Target, ScratchIntent},
    resource_target::Prepared,
    security::{self, AclPolicy},
    session_artifacts,
    session_commands::Session,
    session_wire::{self, Channel, Reply, Request, reply::Outcome, request::Operation},
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    os::windows::io::OwnedHandle,
    path::Path,
    time::{Duration, Instant},
};

pub(crate) const SCHEMA: u32 = 1;
pub(crate) fn nonce(installation: &str, resource: &str) -> Result<String> {
    crate::processes::resource_job::name(installation, resource)?;
    Ok(digest(&("native.debugger.persona.v1", installation, resource))?[..48].into())
}
struct End {
    request: Request,
    result: Value,
}
type Controller = Channel<tokio::io::Stdin, tokio::io::Stdout>;

async fn watch(
    cancel: &Path,
    deadline: Instant,
    output: &CaptureWatch,
    diagnostics: &CaptureWatch,
    helper: &OwnedProcess,
) -> Result<()> {
    loop {
        ensure!(!helper.exited(), "owned MCP server exited");
        ensure!(!cancel.try_exists()?, "native debugger was cancelled");
        ensure!(
            !output.failed() && !diagnostics.failed(),
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
    let value = json!({"session_id":target.resource_id,"worker_session_id":target.resource_id,
        "native_owner":target.owner,"engine":"dbgeng","state":state,
        "transcript":transcript,"response":result,"production_execution":false});
    Ok(Reply {
        schema: session_wire::SCHEMA,
        resource: target.resource_id.clone(),
        sequence: request.sequence,
        request_sha256: digest(target)?,
        outcome: Some(Outcome::ResultJson(serde_json::to_vec(&value)?)),
    })
}

struct Serving<'a> {
    prepared: &'a Prepared,
    request: &'a Target,
    root: &'a Path,
    cancel: &'a Path,
    helper: &'a OwnedProcess,
    diagnostics: &'a CaptureWatch,
}
async fn serve(
    channel: &mut Controller,
    client: Client,
    target: &mut CapturedProcess,
    context: Serving<'_>,
) -> Result<End> {
    let Serving {
        prepared,
        request,
        root,
        cancel,
        helper,
        diagnostics,
    } = context;
    let output = target.watch();
    let deadline = Instant::now() + Duration::from_secs(request.timeout_secs);
    let mut sequence = 0;
    // Only Open may use this unattached client. Later commands own a session.
    let mut unopened = Some(client);
    let mut session: Option<Session> = None;
    loop {
        ensure!(!helper.exited(), "owned MCP server exited");
        let next: Request = tokio::select! {
            next=channel.receive()=>next?.context("native controller channel closed")?,
            stopped=watch(cancel,deadline,&output,diagnostics,helper)=>{stopped?;unreachable!()},
        };
        ensure!(
            next.schema == session_wire::SCHEMA
                && next.resource == request.resource_id
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
                stopped=watch(cancel,deadline,&output,diagnostics,helper)=>{stopped?;unreachable!()},
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
                    let mut opened = client.attach_owned(target.process_mut()).await?;
                    let id = opened["session_id"]
                        .as_str()
                        .context("MCP opener omitted its session handle")?
                        .to_owned();
                    let identity = prepared
                        .identity
                        .as_ref()
                        .context("admitted debugger identity missing")?;
                    let mut symbols =
                        json!({"session_id":id,"path":prepared.symbols(),"append":false});
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
                        "correspondence":"pe_and_pdb_metadata","loaded_pdb_bytes_verified":false});
                    session = Some(Session::new(client, id));
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
            }
        };
        let result = tokio::select! {
            result=operation=>result?,
            stopped=watch(cancel,deadline,&output,diagnostics,helper)=>{stopped?;unreachable!()},
        };
        let transcript =
            session_artifacts::snapshot(root, &request.resource_id, sequence, &digest(request)?)?;
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
    let id = &request.resource_id;
    let root = &host.root;
    ensure!(
        root.join("metadata/resource-requests")
            .join(format!("{id}.debug.json"))
            .canonicalize()?
            == request_path.canonicalize()?
            && request.owner.identity.installation_id == installation
            && request.owner.identity.package_sha256 == host.manifest.sha256(),
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
    let debugger_nonce = nonce(installation, id)?;
    let (mut debugger, _) = crate::resource_host::create_persona_named(
        &host.deployment,
        root,
        Role::Debugger,
        &debugger_nonce,
        Some(id),
    )?;
    deployment::publish_json(
        &root
            .join("metadata")
            .join(format!("resource-scratch-{debugger_nonce}.json")),
        &ScratchIntent {
            schema: crate::resource_process::SCHEMA,
            installation_id: installation.into(),
            resource_id: id.clone(),
            appcontainer_sid: debugger.sid_text()?,
            logon_sid: debugger.logon_sid().into(),
        },
        &private,
    )?;
    let debugger_directory = security::ResourceDirectory::create(
        &host.resources.join(&debugger_nonce),
        &debugger.sid_text()?,
        debugger.logon_sid(),
    )?;
    let debugger_path = debugger_directory.path().to_owned();
    let mut prepared = Prepared::open(&host, &request, Some(&debugger))?;
    let package = host.deployment.package(root);
    let executable = package.join(
        host.manifest
            .manifest()
            .entrypoints
            .windbg_mcp
            .as_ref()
            .context("package has no WinDbg MCP backend")?,
    );
    let (input, writer) = std::io::pipe()?;
    let (reader, output) = std::io::pipe()?;
    let input = fs::File::from(OwnedHandle::from(input));
    let output = fs::File::from(OwnedHandle::from(output));
    let (diagnostic_reader, diagnostic_writer) = std::io::pipe()?;
    let log = fs::File::from(OwnedHandle::from(diagnostic_writer));
    let transcript = session_artifacts::create(root, id)?;
    let mut environment =
        crate::resource_host::scratch_environment(&host.environment, &debugger_path);
    environment.insert(
        "_NT_SYMBOL_PATH".into(),
        prepared.symbols().as_os_str().to_owned(),
    );
    environment.insert(
        "_NT_ALT_SYMBOL_PATH".into(),
        prepared.symbols().as_os_str().to_owned(),
    );
    let helper = Spawn {
        executable: &executable,
        arguments: &["--tools".into(), "session,inspect,exec,crash".into()],
        directory: &debugger_path,
        environment: &environment,
        log: &output,
        input: &input,
    }
    .sandbox_streamed(
        &debugger,
        debugger_directory,
        &log,
        ResourceLimits::new(4, 1024 * 1024 * 1024)?,
    )?;
    drop((input, output));
    let input = prepared
        .input
        .take()
        .map(|input| Box::new(input) as Box<dyn Read + Send>);
    let mut target = PreparedIo::new(request.output_limit)?.launch(input, |io| {
        prepared.launch_with_debugger(
            io,
            ResourceLimits::new(request.processes, request.memory_bytes)?,
            Some(&debugger),
        )
    })?;
    drop(log);
    let diagnostics = CapturedStream::start(diagnostic_reader, 1024 * 1024)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let mut channel = Channel::new(tokio::io::stdin(), tokio::io::stdout());
    let outcome = runtime.block_on(async {
        let client = Client::connect(
            tokio::fs::File::from_std(fs::File::from(OwnedHandle::from(reader))),
            tokio::fs::File::from_std(fs::File::from(OwnedHandle::from(writer))),
            transcript,
        )
        .await?;
        serve(
            &mut channel,
            client,
            &mut target,
            Serving {
                prepared: &prepared,
                request: &request,
                root,
                cancel: &cancel,
                helper: &helper,
                diagnostics: diagnostics.watch(),
            },
        )
        .await
    });
    let helper_stopped = helper.stop(Duration::from_secs(10));
    let target_stopped = target.wait_with_cancel(Duration::from_secs(10), || Ok(true));
    drop(helper);
    // Always shut down Tokio, even when cleanup or evidence publication fails.
    let sent = (|| -> Result<()> {
        helper_stopped?;
        let captured = target_stopped?;
        let diagnostics = diagnostics.finish()?;
        let identity = serde_json::to_value(&prepared.identity)?;
        prepared.close()?;
        debugger.close()?;
        private.verify(&debugger_path, true)?;
        crate::resource_cleanup::remove_reclaimed(&debugger_path)?;
        let result_directory = root.join("metadata/resource-results");
        private.publish(
            &result_directory.join(format!("{id}.debug-stderr")),
            &diagnostics,
        )?;
        private.publish(
            &result_directory.join(format!("{id}.stdout")),
            &captured.stdout,
        )?;
        private.publish(
            &result_directory.join(format!("{id}.stderr")),
            &captured.stderr,
        )?;
        let proof = json!({"schema":SCHEMA,"scope":"native_debugger_host","resource_id":id,
        "owner":request.owner,"request_sha256":digest(&request)?,"identity":identity,"outcome":if outcome.is_ok(){"closed"}else{"unknown"},
        "stdout_sha256":hex::encode(Sha256::digest(&captured.stdout)),"stderr_sha256":hex::encode(Sha256::digest(&captured.stderr)),
        "jobs_empty":true,"profiles_removed":true,"scratch_removed":true,"production_execution":false});
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
