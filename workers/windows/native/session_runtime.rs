//! Service-side ownership of one persistent debugger host. A pending exchange
//! takes only the pipe channel out of the resource lock; Job ownership stays.
use crate::{
    deployment,
    inputs::digest,
    processes::{ExitSignal, Spawn, SystemHost},
    resource_process::Request as Target,
    security::AclPolicy,
    session_wire::{self, Channel, Request, request::Operation},
};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{
    fs::File,
    os::windows::io::OwnedHandle,
    path::Path,
    time::{Duration, Instant},
};

type Pipes = Channel<tokio::fs::File, tokio::fs::File>;
const CALL_TIMEOUT: Duration = Duration::from_secs(75);

pub(crate) struct Live {
    // Drain the Job before dropping channel handles if the service is torn down.
    host: SystemHost,
    channel: Option<Pipes>,
    pub request: Target,
    pub request_hash: String,
    sequence: u64,
    deadline: Option<Instant>,
    pub failed: Option<String>,
}
pub(crate) struct Pending {
    channel: Pipes,
    request: Request,
    hash: String,
    exit: Option<ExitSignal>,
}
pub(crate) struct Completed {
    channel: Pipes,
    request: Request,
    pub result: Result<Value>,
}
impl Pending {
    pub(crate) async fn exchange(mut self) -> Completed {
        let result = tokio::time::timeout(CALL_TIMEOUT, async {
            let value = self.channel.exchange(&self.request, &self.hash).await?;
            if let Some(exit) = &self.exit {
                exit.wait().await?;
            }
            Ok(value)
        })
        .await
        .context("native debugger exchange timed out; reconcile its owned host")
        .and_then(|r| r);
        Completed {
            channel: self.channel,
            request: self.request,
            result,
        }
    }
}
impl Live {
    pub(crate) fn spawn(root: &Path, executable: &Path, request: Target) -> Result<Self> {
        request.validate()?;
        let installation = &request.owner.identity.installation_id;
        let id = &request.resource_id;
        let private = AclPolicy::private_vm();
        let request_path = root
            .join("metadata/resource-requests")
            .join(format!("{id}.debug.json"));
        deployment::publish_json(&request_path, &request, &private)?;
        let log = private.create_file(
            &root
                .join("metadata/resource-requests")
                .join(format!("{id}.debug.log")),
        )?;
        let (input, writer) = std::io::pipe()?;
        let (reader, output) = std::io::pipe()?;
        let input = File::from(OwnedHandle::from(input));
        let output = File::from(OwnedHandle::from(output));
        let environment = crate::environment::system(root)?;
        let host = Spawn {
            executable,
            arguments: &[
                "resource-debugger".into(),
                installation.into(),
                root.join("metadata/deployment.json").into_os_string(),
                request_path.into_os_string(),
            ],
            directory: root,
            environment: &environment,
            log: &output,
            input: &input,
        }
        .system_host_streamed(installation, id, &log)?;
        drop((input, output));
        let reader = tokio::fs::File::from_std(File::from(OwnedHandle::from(reader)));
        let writer = tokio::fs::File::from_std(File::from(OwnedHandle::from(writer)));
        Ok(Self {
            host,
            channel: Some(Channel::new(reader, writer)),
            request_hash: digest(&request)?,
            request,
            sequence: 0,
            deadline: None,
            failed: None,
        })
    }
    pub(crate) fn prepare(&mut self, operation: Operation) -> Result<Pending> {
        ensure!(
            self.failed.is_none() && !self.host.exited(),
            "native debugger host is not live"
        );
        ensure!(
            self.sequence < 4096,
            "native debugger call budget exhausted"
        );
        let exit = matches!(&operation, Operation::Close(_))
            .then(|| self.host.exit_signal())
            .transpose()?;
        let channel = self
            .channel
            .take()
            .context("native debugger already has an in-flight exchange")?;
        self.sequence += 1;
        self.deadline = Some(Instant::now() + CALL_TIMEOUT);
        Ok(Pending {
            channel,
            request: Request {
                schema: session_wire::SCHEMA,
                resource: self.request.resource_id.clone(),
                sequence: self.sequence,
                operation: Some(operation),
            },
            hash: self.request_hash.clone(),
            exit,
        })
    }
    pub(crate) fn complete(&mut self, completed: Completed) -> Result<Value> {
        ensure!(
            self.channel.is_none()
                && completed.request.resource == self.request.resource_id
                && completed.request.sequence == self.sequence,
            "native debugger completion is stale"
        );
        self.deadline = None;
        let result = completed.result.and_then(|value| {
            ensure!(
                value["transcript"]
                    == format!(
                        "native-results/{}/transcript-{}",
                        self.request.resource_id, self.sequence
                    ),
                "native debugger transcript sequence differs"
            );
            Ok(value)
        });
        match result {
            Ok(value) => {
                self.channel = Some(completed.channel);
                Ok(value)
            }
            Err(error) => {
                self.failed = Some(format!("{error:#}").chars().take(2048).collect());
                Err(error)
            }
        }
    }
    pub(crate) fn needs_recovery(&self) -> bool {
        self.failed.is_some()
            || self.channel.is_some() && self.host.exited()
            || self
                .deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
    }
    pub(crate) fn busy(&self) -> bool {
        self.channel.is_none()
    }
    pub(crate) fn finish(&self) -> Result<()> {
        ensure!(
            self.host.stop(Duration::from_secs(10))? == 0,
            "native debugger host did not confirm clean exit"
        );
        Ok(())
    }
}
