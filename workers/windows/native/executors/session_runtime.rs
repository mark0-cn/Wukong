//! Service-side ownership of one persistent session host. A pending exchange
//! takes only the pipe channel out of the resource lock; Job ownership stays.
use crate::{
    artifacts::inputs::digest,
    executors::session_request::Request as Target,
    executors::session_wire::{self, Channel, Request, request::Operation},
    isolation::processes::{ExitSignal, Spawn, SystemHost},
    isolation::security::AclPolicy,
};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{
    fs::File,
    os::windows::io::OwnedHandle,
    path::Path,
    time::{Duration, Instant},
};

type Pipes = Channel<
    crate::rpc::stream_io::Stream<std::io::PipeReader>,
    crate::rpc::stream_io::Stream<std::io::PipeWriter>,
>;
const CALL_TIMEOUT: Duration = Duration::from_secs(75);

fn validate_operation(
    kind: crate::resources::models::ResourceKind,
    read_only: bool,
    operation: &Operation,
) -> Result<()> {
    use crate::resources::models::ResourceKind::{Debugger, Interactive};
    ensure!(
        matches!(kind, Debugger | Interactive),
        "operation requires a session resource"
    );
    ensure!(
        match operation {
            Operation::Write(write) => {
                write.validate()?;
                kind == Interactive
            }
            Operation::Inspect(_) | Operation::Control(_) => kind == Debugger,
            Operation::Open(_) | Operation::Close(_) | Operation::Profile(_) => true,
        },
        "native session operation does not match its admitted kind"
    );
    ensure!(
        !read_only || !matches!(operation, Operation::Control(_)),
        "dump sessions permit inspection and close only"
    );
    Ok(())
}

pub(crate) struct Live {
    // Drain the Job before dropping channel handles if the service is torn down.
    host: SystemHost,
    channel: Option<Pipes>,
    pub request: Target,
    pub request_hash: String,
    sequence: u64,
    deadline: Option<Instant>,
    cancel_requested: bool,
    pub failed: Option<String>,
}
pub(crate) struct Pending {
    channel: Pipes,
    request: Request,
    hash: String,
    exit: Option<ExitSignal>,
    timeout: Duration,
}
pub(crate) struct Completed {
    channel: Pipes,
    request: Request,
    pub result: Result<Value>,
}
impl Pending {
    pub(crate) async fn exchange(mut self) -> Completed {
        let result = tokio::time::timeout(self.timeout, async {
            let value = self.channel.exchange(&self.request, &self.hash).await?;
            if let Some(exit) = &self.exit
                && (matches!(self.request.operation, Some(Operation::Close(_)))
                    || value["state"] == "exited")
            {
                exit.wait().await?;
            }
            Ok(value)
        })
        .await
        .context("native session exchange timed out; reconcile its owned host")
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
        let installation = &request.target.owner.identity.installation_id;
        let id = &request.target.resource_id;
        let prefix = crate::executors::session_request::prefix(request.kind())?;
        let private = AclPolicy::private_vm();
        let request_path = root
            .join("metadata/resource-requests")
            .join(format!("{id}.{prefix}.json"));
        crate::artifacts::io::publish_json(&request_path, &request, &private)?;
        let log = private.create_file(
            &root
                .join("metadata/resource-requests")
                .join(format!("{id}.{prefix}.log")),
        )?;
        let (input, writer) = std::io::pipe()?;
        let (reader, output) = std::io::pipe()?;
        let input = File::from(OwnedHandle::from(input));
        let output = File::from(OwnedHandle::from(output));
        let environment = crate::isolation::environment::system(root)?;
        let host = Spawn {
            executable,
            arguments: &[
                "resource-session".into(),
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
        let reader = crate::rpc::stream_io::reader(reader);
        let writer = crate::rpc::stream_io::writer(writer);
        Ok(Self {
            host,
            channel: Some(Channel::new(reader, writer)),
            request_hash: digest(&request)?,
            request,
            sequence: 0,
            deadline: None,
            cancel_requested: false,
            failed: None,
        })
    }
    pub(crate) fn prepare(&mut self, operation: Operation) -> Result<Pending> {
        validate_operation(self.request.kind(), self.request.read_only(), &operation)?;
        ensure!(
            self.failed.is_none() && !self.cancel_requested && !self.host.exited(),
            "native session host is not live"
        );
        ensure!(self.sequence < 4096, "native session call budget exhausted");
        let timeout = match &operation {
            Operation::Write(write) => {
                Duration::from_secs(u64::from(write.read_timeout_seconds) + 30)
            }
            _ => CALL_TIMEOUT,
        };
        let exit = matches!(&operation, Operation::Close(_) | Operation::Write(_))
            .then(|| self.host.exit_signal())
            .transpose()?;
        let channel = self
            .channel
            .take()
            .context("native session already has an in-flight exchange")?;
        self.sequence += 1;
        self.deadline = Some(Instant::now() + timeout);
        Ok(Pending {
            channel,
            request: Request {
                schema: session_wire::SCHEMA,
                resource: self.request.target.resource_id.clone(),
                sequence: self.sequence,
                operation: Some(operation),
            },
            hash: self.request_hash.clone(),
            exit,
            timeout,
        })
    }
    pub(crate) fn complete(&mut self, completed: Completed) -> Result<Value> {
        ensure!(
            self.channel.is_none()
                && completed.request.resource == self.request.target.resource_id
                && completed.request.sequence == self.sequence,
            "native session completion is stale"
        );
        self.deadline = None;
        let result = completed.result.and_then(|value| {
            ensure!(
                !self.cancel_requested,
                "native session was cancelled while its reply was in flight"
            );
            ensure!(
                value["transcript"]
                    == format!(
                        "native-results/{}/transcript-{}",
                        self.request.target.resource_id, self.sequence
                    ),
                "native session transcript sequence differs"
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
            || (self.channel.is_some() || self.cancel_requested) && self.host.exited()
            || self
                .deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
    }
    pub(crate) fn request_cancel(&mut self) {
        if !self.cancel_requested {
            self.cancel_requested = true;
            let grace = Instant::now() + Duration::from_secs(5);
            self.deadline = Some(self.deadline.map_or(grace, |deadline| deadline.min(grace)));
        }
    }
    pub(crate) fn busy(&self) -> bool {
        self.channel.is_none()
    }
    pub(crate) fn finish(&self) -> Result<()> {
        ensure!(
            self.host.stop(Duration::from_secs(10))? == 0,
            "native session host did not confirm clean exit"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::models::ResourceKind;
    #[test]
    fn shared_profile_operation_preserves_session_kind_and_dump_permissions() -> Result<()> {
        let profile = Operation::Profile(session_wire::Profile {});
        for kind in [ResourceKind::Debugger, ResourceKind::Interactive] {
            validate_operation(kind, false, &profile)?;
        }
        validate_operation(ResourceKind::Debugger, true, &profile)?;
        assert!(validate_operation(ResourceKind::Process, false, &profile).is_err());
        let control = Operation::Control(session_wire::Control::default());
        assert!(validate_operation(ResourceKind::Debugger, true, &control).is_err());
        assert!(validate_operation(ResourceKind::Interactive, false, &control).is_err());
        let write = Operation::Write(session_wire::Write {
            data: vec![],
            read_timeout_seconds: 1,
        });
        assert!(validate_operation(ResourceKind::Debugger, false, &write).is_err());
        Ok(())
    }
}
