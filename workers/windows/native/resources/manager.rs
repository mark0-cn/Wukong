//! Serializes reservations and commits; external waits never retain the state/input locks.
//!
//! This manager is the resource-layer boundary shared by the host and the RPC
//! entry: the host builds the verified runtime context, initializes this
//! manager, polls it during startup, and asks it to settle owned work during
//! shutdown; the RPC entry only holds a handle for admission and dispatch.
//! Startup, process draining and recovery waits run in owned tasks outside the
//! state lock, so a slow external wait never blocks status queries, renewal or
//! cancellation. A failed or unconfirmed cleanup keeps its journal blocker.
use crate::{
    artifacts::inputs::Inputs,
    core::Owner,
    host::context::RuntimeContext,
    isolation::security::AclPolicy,
    resources::journal::Journal,
    resources::state::{Dispatch, ResourceState},
};
use anyhow::{Context, Result, ensure};
use serde_json::{Map, Value};
use std::{path::Path, time::Duration};
use tokio::sync::{Mutex, OwnedMutexGuard};
use wukong_worker_protocol::WorkerIdentity;

pub(crate) struct ResourceManager {
    state: Mutex<ResourceState>,
}
impl ResourceManager {
    #[cfg(test)]
    pub(crate) async fn test_state(&self) -> tokio::sync::MutexGuard<'_, ResourceState> {
        self.state.lock().await
    }
    #[cfg(test)]
    pub(crate) fn test_blocking_state(&self) -> tokio::sync::MutexGuard<'_, ResourceState> {
        self.state.blocking_lock()
    }

    pub(crate) fn open(
        metadata: &Path,
        identity: WorkerIdentity,
        policy: AclPolicy,
    ) -> Result<Self> {
        Ok(Self {
            state: Mutex::new(ResourceState::new(Journal::with_policy(
                metadata, identity, policy,
            )?)),
        })
    }
    /// Host-only boundary: the host hands in the context it verified after
    /// deployment identity, platform and protected-directory checks.
    pub(crate) fn initialize(&self, context: RuntimeContext) -> Result<()> {
        self.state
            .try_lock()
            .context("resource manager busy during initialization")?
            .initialize(context)
    }
    pub(crate) async fn initialized(&self) -> bool {
        self.state.lock().await.initialized()
    }
    /// Admission barrier, shared with the host's shutdown verification: the
    /// journal must have no unfinished resource or effect. Never waits for an
    /// external operation while another owner holds the state lock.
    pub(crate) fn idle(&self) -> Result<bool> {
        match self.state.try_lock() {
            Ok(state) => state.idle(),
            Err(_) => Ok(false),
        }
    }
    /// Status polling: advance owned work and report whether the journal is
    /// settled. Host startup, the service keep-alive and RPC queries all
    /// observe this same state; a blocked observation keeps the blocker.
    pub(crate) fn poll(&self, expired: bool) -> Result<bool> {
        let Ok(mut state) = self.state.try_lock() else {
            return Ok(false);
        };
        if state.initialized() {
            if expired {
                state.cancel_active()?;
            }
            state.poll()?;
        }
        state.idle()
    }
    /// Cancellation entry: request the stop of every owned resource without
    /// releasing its journal blocker; confirmed cleanup still commits later.
    pub(crate) async fn cancel(&self) -> Result<()> {
        self.state.lock().await.cancel_active()
    }
    /// Host shutdown use case: cancel owned work, then verify leftover cleanup
    /// within the host's deadline. Unconfirmed cleanup raises the existing
    /// refusal and leaves its resource blocker for a later recovery attempt.
    pub(crate) async fn shutdown(
        &self,
        deadline: tokio::time::Instant,
        expired: impl Fn() -> bool,
    ) -> Result<()> {
        self.cancel().await?;
        while !self.poll(expired())? {
            ensure!(
                tokio::time::Instant::now() < deadline,
                "native resource cleanup remains pending during service stop"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Ok(())
    }
    pub(crate) async fn execute(
        &self,
        inputs: Option<OwnedMutexGuard<Inputs>>,
        owner: &Owner,
        operation: super::operation::Operation,
        params: &Map<String, Value>,
        mut revalidate: impl FnMut() -> Result<()>,
    ) -> Result<Value> {
        let prepared = {
            let mut state = self.state.lock().await;
            // Recheck the lease after either independent lock has been acquired.
            revalidate()?;
            state.prepare(owner, operation, params)?
        };
        let mut inputs = inputs;
        if !matches!(prepared, Dispatch::Admission(_)) {
            drop(inputs.take());
        }
        let mut dispatch = prepared;
        loop {
            match dispatch {
                Dispatch::Admission(plan) => {
                    let store = inputs.take().context("execution input store missing")?;
                    let actor = owner.clone();
                    let (store, result) = tokio::task::spawn_blocking(move || {
                        let result = plan.prepare(&store, &actor);
                        (store, result)
                    })
                    .await
                    .context("input preparation interrupted")?;
                    let prepared = result?;
                    let mut state = self.state.lock().await;
                    revalidate()?;
                    dispatch = state.reserve_prepared(owner, prepared)?;
                    drop(store);
                }

                Dispatch::Starting(pending) => {
                    let status = tokio::time::timeout(Duration::from_secs(300), async {
                        let mut ticks = tokio::time::interval(Duration::from_millis(50));
                        loop {
                            ticks.tick().await;
                            if let Some(status) = self.state.lock().await.poll_started(&pending)? {
                                return Ok::<_, anyhow::Error>(status);
                            }
                        }
                    })
                    .await
                    .context(
                        "native launch observation timed out; reconcile the original resource",
                    )??;
                    dispatch = pending.complete(status)?;
                }
                Dispatch::Ready(value) => return Ok(value),
                Dispatch::Session(pending) => {
                    let completed = pending.exchange().await;
                    return self.state.lock().await.complete_session(completed);
                }
                pending @ (Dispatch::Analysis(_) | Dispatch::Command(_)) => {
                    return tokio::time::timeout(Duration::from_secs(300), async move {
                        let mut ticks = tokio::time::interval(Duration::from_millis(100));
                        loop {
                            ticks.tick().await;
                            let mut state = self.state.lock().await;
                            let value = match &pending {
                                Dispatch::Analysis(pending) => state.poll_analysis(pending)?,
                                Dispatch::Command(pending) => state.poll_command(pending)?,
                                _ => unreachable!(),
                            };
                            if let Some(value) = value {
                                return Ok(value);
                            }
                        }
                    })
                    .await
                    .context("resource observation timed out; reconcile its original request")?;
                }
            }
        }
    }
}
