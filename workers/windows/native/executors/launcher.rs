//! Process executor returns owned controls. It cannot read or mutate the resource ledger.
use crate::{
    executors::resource_process::Request,
    host::context::RuntimeContext,
    isolation::processes::{Spawn, SystemHost},
    isolation::security::AclPolicy,
};
use anyhow::Result;
use std::{
    fs,
    time::{Duration, Instant},
};
pub(crate) struct Running {
    pub(crate) request: Request,
    pub(crate) host: SystemHost,
    pub(crate) deadline: Instant,
    pub(crate) failed: Option<String>,
}
pub(crate) fn start(context: RuntimeContext, request: Request) -> Result<Running> {
    let resource = request.resource_id.clone();
    let private = AclPolicy::private_vm();
    let path = context.artifacts.request_path(&resource, "json")?;
    crate::artifacts::io::publish_json(&path, &request, &private)?;
    let log = private.create_file(&context.artifacts.request_path(&resource, "log")?)?;
    let input = fs::File::open("NUL")?;
    let environment = crate::isolation::environment::system(&context.root)?;
    let host = Spawn {
        executable: &context.executable,
        arguments: &[
            "resource-execute".into(),
            context.installation.clone().into(),
            context
                .root
                .join("metadata/deployment.json")
                .into_os_string(),
            path.into_os_string(),
        ],
        directory: &context.root,
        environment: &environment,
        log: &log,
        input: &input,
    }
    .system_host(&context.installation, &resource)?;
    Ok(Running {
        deadline: Instant::now()
            + Duration::from_secs(
                request
                    .replay
                    .as_ref()
                    .and_then(|replay| replay.configuration.minimization.as_ref())
                    .map_or_else(
                        || {
                            (request.timeout_secs + 120)
                                * u64::from(
                                    request
                                        .replay
                                        .as_ref()
                                        .map_or(1, |replay| replay.configuration.trials),
                                )
                        },
                        |minimize| minimize.timeout_secs + 120,
                    ),
            ),
        request,
        host,
        failed: None,
    })
}

pub(crate) enum Launched {
    Process(Box<Running>),
    Session(
        Box<crate::executors::session_runtime::Live>,
        Box<crate::executors::session_runtime::Completed>,
    ),
}
pub(crate) async fn launch(
    context: RuntimeContext,
    target: crate::executors::plan::Target,
) -> Result<Launched> {
    use crate::executors::{plan::Target, session_runtime, session_wire};
    use anyhow::Context;
    match target {
        Target::Process(request) => tokio::task::spawn_blocking(move || start(context, request))
            .await
            .context("process launcher interrupted")?
            .map(|running| Launched::Process(Box::new(running))),
        Target::Session(request) => {
            let mut live = tokio::task::spawn_blocking(move || {
                session_runtime::Live::spawn(&context.root, &context.executable, request)
            })
            .await
            .context("session launcher interrupted")??;
            let exchange = live
                .prepare(session_wire::request::Operation::Open(
                    session_wire::Open {},
                ))?
                .exchange()
                .await;
            Ok(Launched::Session(Box::new(live), Box::new(exchange)))
        }
    }
}
