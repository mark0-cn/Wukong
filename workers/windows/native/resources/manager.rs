//! Serializes reservations and commits; external waits never retain the state/input locks.
use crate::{
    artifacts::inputs::{Inputs, Owner},
    host::context::RuntimeContext,
    isolation::security::AclPolicy,
    resources::journal::Journal,
    resources::state::{Dispatch, ResourceState},
};
use anyhow::{Context, Result};
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
    pub(crate) fn initialize(&self, context: RuntimeContext) -> Result<()> {
        self.state
            .try_lock()
            .context("resource manager busy during initialization")?
            .initialize(context)
    }
    pub(crate) async fn initialized(&self) -> bool {
        self.state.lock().await.initialized()
    }
    pub(crate) fn idle(&self) -> Result<bool> {
        match self.state.try_lock() {
            Ok(state) => state.idle(),
            Err(_) => Ok(false),
        }
    }
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
    pub(crate) async fn cancel(&self) -> Result<()> {
        self.state.lock().await.cancel_active()
    }
    pub(crate) async fn execute(
        &self,
        inputs: Option<OwnedMutexGuard<Inputs>>,
        owner: &Owner,
        method: &str,
        params: &Map<String, Value>,
        mut revalidate: impl FnMut() -> Result<()>,
    ) -> Result<Value> {
        let prepared = {
            let mut state = self.state.lock().await;
            // Recheck the lease after either independent lock has been acquired.
            revalidate()?;
            state.prepare_rpc(owner, method, params)?
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
