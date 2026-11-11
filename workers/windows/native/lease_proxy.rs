//! Service-owned admission fences. Python retains its own resource checks and
//! receipt ledger, but cannot issue or reset the controller's outer lease.
use crate::{accounts, deployment, security::AclPolicy};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::{
    fs::{self, File, OpenOptions},
    future::Future,
    os::windows::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const TTL: Duration = Duration::from_secs(120);
const MAX_EPOCHS: u64 = 100_000;

#[derive(Debug)]
struct Rejected {
    kind: &'static str,
    message: &'static str,
}
impl std::fmt::Display for Rejected {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str(self.message)
    }
}
impl std::error::Error for Rejected {}
fn require(condition: bool, kind: &'static str, message: &'static str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(Rejected { kind, message }.into())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Identity {
    pub protocol: u32,
    pub installation_id: String,
    pub boot_id: String,
    pub package_sha256: String,
}
impl Identity {
    pub fn from_readiness(value: Value, installation: &str, package: &str) -> Result<Self> {
        let identity: Self = serde_json::from_value(value)?;
        let boot = identity.boot_id.strip_prefix("windows:").unwrap_or("");
        let (entry, sequence) = boot
            .split_once(':')
            .context("invalid native boot binding")?;
        ensure!(
            identity.protocol == 2
                && identity.installation_id == installation
                && identity.package_sha256 == package
                && lower_hex(entry, 32)
                && entry.bytes().any(|b| b != b'0')
                && sequence.parse::<u32>()?.to_string() == sequence,
            "native readiness identity differs from the verified deployment"
        );
        Ok(identity)
    }
}
fn lower_hex(value: &str, size: usize) -> bool {
    value.len() == size
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// Each successfully created name reserves its epoch before it can be issued.
/// Corrupt/partial records fail closed, and an exclusive handle fences another
/// writer. These records live outside the metadata worker's writable directory.
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
                serde_json::from_slice(&deployment::read_bytes(&entry.path(), 256)?)?;
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
        deployment::write_new_json(
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

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Lease {
    epoch: u64,
    run_id: String,
    owner_id: String,
    token: String,
    boot_id: String,
    expires_at: f64,
}
impl Lease {
    fn same_fence(&self, other: &Self) -> bool {
        self.epoch == other.epoch
            && self.run_id == other.run_id
            && self.owner_id == other.owner_id
            && self.token == other.token
            && self.boot_id == other.boot_id
    }
}
struct Current {
    lease: Lease,
    deadline: Instant,
    inner: Value,
}
struct State {
    epochs: Epochs,
    current: Option<Current>,
    inflight: usize,
    uncertain: bool,
    transition: bool,
}
pub(crate) struct LeaseProxy {
    identity: Identity,
    state: Arc<Mutex<State>>,
    controls: tokio::sync::Mutex<()>,
    started: Instant,
}
struct Admission {
    state: Arc<Mutex<State>>,
    completed: bool,
}
impl Drop for Admission {
    fn drop(&mut self) {
        let mut state = self.state.lock().expect("native lease mutex poisoned");
        state.inflight -= 1;
        // Losing a reply does not prove the worker operation stopped. Only a
        // worker-epoch teardown can reconcile this conservative idle blocker.
        state.uncertain |= !self.completed;
    }
}
struct Transition(Arc<Mutex<State>>);
impl Drop for Transition {
    fn drop(&mut self) {
        self.0
            .lock()
            .expect("native lease mutex poisoned")
            .transition = false;
    }
}
impl LeaseProxy {
    pub fn open(identity: Identity, metadata: &Path) -> Result<Self> {
        Self::with_policy(identity, metadata, AclPolicy::private_vm())
    }
    fn with_policy(identity: Identity, metadata: &Path, policy: AclPolicy) -> Result<Self> {
        Ok(Self {
            identity,
            state: Arc::new(Mutex::new(State {
                epochs: Epochs::open(metadata, policy)?,
                current: None,
                inflight: 0,
                uncertain: false,
                transition: false,
            })),
            controls: tokio::sync::Mutex::new(()),
            started: Instant::now(),
        })
    }
    fn binding(&self, params: &Map<String, Value>) -> Result<()> {
        let binding: Identity = serde_json::from_value(
            params
                .get("_binding")
                .cloned()
                .context("missing native binding")?,
        )?;
        require(
            binding == self.identity,
            "binding_changed",
            "worker installation, boot, package or protocol binding changed",
        )?;
        Ok(())
    }
    fn validate<'a>(&self, state: &'a State, token: &Value) -> Result<&'a Current> {
        let token: Lease = serde_json::from_value(token.clone())?;
        let current = state.current.as_ref().ok_or(Rejected {
            kind: "lease_lost",
            message: "native lease is not held",
        })?;
        require(
            current.deadline > Instant::now() && current.lease.same_fence(&token),
            "lease_lost",
            "native lease expired or was fenced by a successor",
        )?;
        Ok(current)
    }
    fn admit(&self, params: &mut Map<String, Value>) -> Result<Admission> {
        self.binding(params)?;
        let mut state = self.state.lock().expect("native lease mutex poisoned");
        require(
            !state.transition,
            "conflict",
            "native lease ownership is changing",
        )?;
        let current = self.validate(
            &state,
            params.get("_lease").context("missing native lease")?,
        )?;
        params.insert("_lease".into(), current.inner.clone());
        state.inflight += 1;
        Ok(Admission {
            state: self.state.clone(),
            completed: false,
        })
    }
    pub async fn call<F, Fut>(&self, bytes: &[u8], upstream: F) -> Result<Vec<u8>>
    where
        F: Fn(Vec<u8>) -> Fut,
        Fut: Future<Output = Result<Vec<u8>>>,
    {
        let mut request: Value = serde_json::from_slice(bytes)?;
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        match self.dispatch(&mut request, upstream).await {
            Ok(reply) => Ok(reply),
            Err(error) => {
                let kind = error.downcast_ref::<Rejected>().map(|e| e.kind).unwrap_or(
                    if error.is::<serde_json::Error>() {
                        "invalid_request"
                    } else {
                        "internal"
                    },
                );
                encode(&json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,
                "message":format!("{error:#}").chars().take(2048).collect::<String>(),
                "data":{"kind":kind}}}))
            }
        }
    }
    async fn dispatch<F, Fut>(&self, request: &mut Value, upstream: F) -> Result<Vec<u8>>
    where
        F: Fn(Vec<u8>) -> Fut,
        Fut: Future<Output = Result<Vec<u8>>>,
    {
        ensure!(
            request["jsonrpc"] == "2.0" && request.is_object(),
            "invalid native RPC envelope"
        );
        let method = request["method"]
            .as_str()
            .context("missing native RPC method")?
            .to_owned();
        ensure!(
            !request.as_object().unwrap().contains_key("auth"),
            "authentication is service-owned"
        );
        if matches!(method.as_str(), "ping" | "doctor") {
            return upstream(encode(request)?).await;
        }
        if !request.get("params").is_some_and(Value::is_object) {
            anyhow::bail!("native RPC parameters must be an object");
        }
        if method == "environment.lease.acquire" {
            return self.acquire(request, upstream).await;
        }
        if method == "environment.lease.release" {
            return self.release(request, upstream).await;
        }
        let mut admission = self.admit(request["params"].as_object_mut().unwrap())?;
        let reply = upstream(encode(request)?).await?;
        // A structured error is also a settled RPC outcome. Transport failures,
        // malformed replies and dropped futures remain uncertain.
        owned_reply(request, &reply)?;
        admission.completed = true;
        Ok(reply)
    }
    async fn acquire<F, Fut>(&self, request: &mut Value, upstream: F) -> Result<Vec<u8>>
    where
        F: Fn(Vec<u8>) -> Fut,
        Fut: Future<Output = Result<Vec<u8>>>,
    {
        let _control = self.controls.lock().await;
        let params = request["params"].as_object().unwrap();
        self.binding(params)?;
        let text = |key| -> Result<String> {
            let value = params
                .get(key)
                .and_then(Value::as_str)
                .context("lease requires run and controller identities")?;
            ensure!(
                !value.is_empty() && value.chars().count() <= 256,
                "lease identity exceeds limit"
            );
            Ok(value.into())
        };
        let (run, owner) = (text("run_id")?, text("owner_id")?);
        let (renew, epoch) = {
            let mut state = self.state.lock().expect("native lease mutex poisoned");
            let current = state
                .current
                .as_ref()
                .filter(|c| c.deadline > Instant::now());
            if let Some(current) = current {
                require(
                    current.lease.run_id == run && current.lease.owner_id == owner,
                    "conflict",
                    "another controller holds the native environment lease",
                )?;
                (true, current.lease.epoch)
            } else {
                require(
                    state.inflight == 0 && !state.uncertain,
                    "conflict",
                    "previous native requests remain active or uncertain; reconcile the worker epoch first",
                )?;
                state.transition = true;
                (false, 0)
            }
        };
        let _transition = (!renew).then(|| Transition(self.state.clone()));
        let mut reply = owned_reply(request, &upstream(encode(request)?).await?)?;
        if reply.get("error").is_some() {
            return encode(&reply);
        }
        let inner: Lease = serde_json::from_value(reply["result"].clone())?;
        ensure!(
            reply["id"] == request["id"]
                && reply["jsonrpc"] == "2.0"
                && inner.run_id == run
                && inner.owner_id == owner
                && inner.boot_id == self.identity.boot_id
                && inner.epoch > 0
                && lower_hex(&inner.token, 32)
                && inner.expires_at.is_finite(),
            "owned worker returned an invalid inner lease"
        );
        let mut state = self.state.lock().expect("native lease mutex poisoned");
        if renew {
            require(
                state
                    .current
                    .as_ref()
                    .is_some_and(|c| c.deadline > Instant::now()),
                "lease_lost",
                "native lease expired during renewal; acquire a fresh fence",
            )?;
        }
        let epoch = if renew {
            epoch
        } else {
            state.epochs.reserve()?
        };
        let token = if renew {
            state
                .current
                .as_ref()
                .context("native lease vanished")?
                .lease
                .token
                .clone()
        } else {
            hex::encode(accounts::random_bytes::<16>()?)
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
            inner: reply["result"].clone(),
        });
        reply["result"] = serde_json::to_value(lease)?;
        encode(&reply)
    }
    async fn release<F, Fut>(&self, request: &mut Value, upstream: F) -> Result<Vec<u8>>
    where
        F: Fn(Vec<u8>) -> Fut,
        Fut: Future<Output = Result<Vec<u8>>>,
    {
        let _control = self.controls.lock().await;
        let params = request["params"].as_object_mut().unwrap();
        self.binding(params)?;
        let epoch = {
            let mut state = self.state.lock().expect("native lease mutex poisoned");
            let current = self.validate(
                &state,
                params
                    .get("lease")
                    .context("missing native release lease")?,
            )?;
            let epoch = current.lease.epoch;
            params.insert("lease".into(), current.inner.clone());
            require(
                state.inflight == 0 && !state.uncertain,
                "conflict",
                "native requests remain active or uncertain",
            )?;
            state.transition = true;
            epoch
        };
        let _transition = Transition(self.state.clone());
        let mut reply = owned_reply(request, &upstream(encode(request)?).await?)?;
        if reply.get("error").is_some() {
            return encode(&reply);
        }
        ensure!(
            reply["jsonrpc"] == "2.0"
                && reply["id"] == request["id"]
                && reply["result"]["released"] == true,
            "invalid owned worker release receipt"
        );
        self.state
            .lock()
            .expect("native lease mutex poisoned")
            .current = None;
        reply["result"] = json!({"released":true,"epoch":epoch});
        encode(&reply)
    }
}
fn encode(value: &Value) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}
fn owned_reply(request: &Value, bytes: &[u8]) -> Result<Value> {
    let reply: Value = serde_json::from_slice(bytes)?;
    ensure!(
        reply["jsonrpc"] == "2.0"
            && reply["id"] == request["id"]
            && (reply.get("result").is_some() ^ reply.get("error").is_some()),
        "invalid owned worker reply"
    );
    Ok(reply)
}

#[cfg(test)]
#[path = "lease_proxy_tests.rs"]
mod tests;
