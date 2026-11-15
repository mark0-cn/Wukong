//! Service-owned run fencing. No transport, Python state or resource dispatch.
use crate::{artifacts::inputs::Owner, host::accounts, isolation::security::AclPolicy};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    fs::{self, File, OpenOptions},
    os::windows::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use wukong_worker_protocol::{EnvironmentLease as Lease, WorkerIdentity as Identity};

const TTL: Duration = Duration::from_secs(120);
const MAX_EPOCHS: u64 = 100_000;
#[derive(Debug)]
pub(crate) struct Rejected {
    pub kind: wukong_worker_protocol::WorkerErrorKind,
    pub message: &'static str,
}
impl std::fmt::Display for Rejected {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str(self.message)
    }
}
impl std::error::Error for Rejected {}
pub(crate) fn require(
    condition: bool,
    kind: wukong_worker_protocol::WorkerErrorKind,
    message: &'static str,
) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(Rejected { kind, message }.into())
    }
}
/// Each successfully created name reserves its epoch before it can be issued.
/// Corrupt/partial records fail closed, and an exclusive handle fences another
/// writer. These records live outside every ordinary tool account's writable directories.
struct Epochs {
    directory: PathBuf,
    policy: AclPolicy,
    high: u64,
    _lock: File,
}
impl Epochs {
    fn open(metadata: &Path, policy: AclPolicy) -> Result<Self> {
        policy.verify(metadata, true)?;
        let directory = metadata.join("lease-epochs");
        if directory.exists() {
            policy.verify(&directory, true)?;
        } else {
            policy.create_directory(&directory)?;
        }
        let lock = directory.join("writer.lock");
        if !lock.exists() {
            policy.create_file(&lock)?.sync_all()?;
        }
        policy.verify(&lock, false)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            .open(lock)?;
        let mut high = 0;
        let mut count = 0;
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            if entry.file_name() == "writer.lock" {
                continue;
            }
            count += 1;
            ensure!(count <= MAX_EPOCHS, "native lease epoch journal is full");
            policy.verify(&entry.path(), false)?;
            let record: Epoch =
                serde_json::from_slice(&crate::artifacts::io::read_bytes(&entry.path(), 256)?)?;
            ensure!(
                record.schema == 1
                    && record.epoch > 0
                    && record.epoch <= MAX_EPOCHS
                    && entry.file_name() == format!("epoch-{:020}.json", record.epoch).as_str(),
                "invalid native lease epoch record"
            );
            high = high.max(record.epoch);
        }
        ensure!(count == high, "native lease epoch journal has a gap");
        Ok(Self {
            directory,
            policy,
            high,
            _lock: lock,
        })
    }
    fn reserve(&mut self) -> Result<u64> {
        let epoch = self
            .high
            .checked_add(1)
            .context("native lease epoch exhausted")?;
        ensure!(epoch <= MAX_EPOCHS, "native lease epoch journal is full");
        crate::artifacts::io::write_new_json(
            &self.directory.join(format!("epoch-{epoch:020}.json")),
            &Epoch { schema: 1, epoch },
            &self.policy,
        )?;
        self.high = epoch;
        Ok(epoch)
    }
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Epoch {
    schema: u32,
    epoch: u64,
}

pub(super) struct Current {
    pub lease: Lease,
    pub deadline: Instant,
}
pub(super) struct State {
    epochs: Epochs,
    pub current: Option<Current>,
    pub inflight: usize,
}
pub(crate) struct Leases {
    identity: Identity,
    pub(super) state: Arc<Mutex<State>>,
    started: Instant,
}
pub(crate) struct Admission {
    state: Arc<Mutex<State>>,
    pub owner: Owner,
}
impl Drop for Admission {
    fn drop(&mut self) {
        self.state.lock().expect("lease mutex poisoned").inflight -= 1;
    }
}
impl Leases {
    pub fn open(identity: Identity, metadata: &Path, policy: AclPolicy) -> Result<Self> {
        Ok(Self {
            identity,
            state: Arc::new(Mutex::new(State {
                epochs: Epochs::open(metadata, policy)?,
                current: None,
                inflight: 0,
            })),
            started: Instant::now(),
        })
    }
    fn binding(&self, params: &Map<String, Value>) -> Result<()> {
        let identity: Identity = serde_json::from_value(
            params
                .get("_binding")
                .cloned()
                .context("missing native binding")?,
        )?;
        require(
            identity == self.identity,
            wukong_worker_protocol::WorkerErrorKind::BindingChanged,
            "worker installation, boot, package or protocol binding changed",
        )
    }
    fn current<'a>(&self, state: &'a State, token: &Value) -> Result<&'a Current> {
        let token: Lease = serde_json::from_value(token.clone())?;
        let current = state.current.as_ref().ok_or(Rejected {
            kind: wukong_worker_protocol::WorkerErrorKind::LeaseLost,
            message: "native lease is not held",
        })?;
        require(
            current.deadline > Instant::now() && current.lease.same_fence(&token),
            wukong_worker_protocol::WorkerErrorKind::LeaseLost,
            "native lease expired or was fenced by a successor",
        )?;
        Ok(current)
    }
    pub fn validate(&self, token: &Value) -> Result<()> {
        self.current(&self.state.lock().expect("lease mutex poisoned"), token)
            .map(|_| ())
    }
    pub fn expired(&self) -> bool {
        self.state
            .lock()
            .expect("lease mutex poisoned")
            .current
            .as_ref()
            .is_some_and(|c| c.deadline <= Instant::now())
    }
    pub fn admit(&self, params: &Map<String, Value>) -> Result<Admission> {
        self.binding(params)?;
        let mut state = self.state.lock().expect("lease mutex poisoned");
        let current = self.current(
            &state,
            params.get("_lease").context("missing native lease")?,
        )?;
        let owner = Owner {
            identity: self.identity.clone(),
            run_id: current.lease.run_id.clone(),
            owner_id: current.lease.owner_id.clone(),
            epoch: current.lease.epoch,
        };
        state.inflight += 1;
        Ok(Admission {
            state: self.state.clone(),
            owner,
        })
    }
    // The idle callback must only try-lock the resource journal. It must never
    // wait while the lease state is held; resource admission rechecks this fence.
    pub fn acquire(
        &self,
        params: &Map<String, Value>,
        idle: impl FnOnce() -> Result<bool>,
    ) -> Result<Lease> {
        self.binding(params)?;
        let text = |key| -> Result<String> {
            let text = params
                .get(key)
                .and_then(Value::as_str)
                .context("lease requires run and controller identities")?;
            ensure!(
                !text.is_empty() && text.len() <= 128,
                "lease identity exceeds limit"
            );
            Ok(text.into())
        };
        let (run, owner) = (text("run_id")?, text("owner_id")?);
        let mut state = self.state.lock().expect("lease mutex poisoned");
        let (epoch, token) = if let Some(current) = state
            .current
            .as_ref()
            .filter(|c| c.deadline > Instant::now())
        {
            require(
                current.lease.run_id == run && current.lease.owner_id == owner,
                wukong_worker_protocol::WorkerErrorKind::Conflict,
                "another controller holds the native environment lease",
            )?;
            (current.lease.epoch, current.lease.token.clone())
        } else {
            require(
                state.inflight == 0 && idle()?,
                wukong_worker_protocol::WorkerErrorKind::Conflict,
                "previous native requests or resources remain active; reconcile ownership first",
            )?;
            (
                state.epochs.reserve()?,
                hex::encode(accounts::random_bytes::<16>()?),
            )
        };
        let lease = Lease {
            epoch,
            run_id: run,
            owner_id: owner,
            token,
            boot_id: self.identity.boot_id.clone(),
            expires_at: self.started.elapsed().as_secs_f64() + TTL.as_secs_f64(),
        };
        state.current = Some(Current {
            lease: lease.clone(),
            deadline: Instant::now() + TTL,
        });
        Ok(lease)
    }
    pub fn release(
        &self,
        params: &Map<String, Value>,
        idle: impl FnOnce() -> Result<bool>,
    ) -> Result<u64> {
        self.binding(params)?;
        let mut state = self.state.lock().expect("lease mutex poisoned");
        let epoch = self
            .current(
                &state,
                params
                    .get("lease")
                    .context("missing native release lease")?,
            )?
            .lease
            .epoch;
        require(
            state.inflight == 0 && idle()?,
            wukong_worker_protocol::WorkerErrorKind::Conflict,
            "native requests or resources remain active",
        )?;
        state.current = None;
        Ok(epoch)
    }
}
