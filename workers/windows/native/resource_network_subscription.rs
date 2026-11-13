//! SDK live notifications avoid depending on the global circular event log.
//! No global collection option or firewall rule is changed by this observer.
use super::*;
use std::{
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Condvar, Mutex},
};

const MAX_RECORDS: usize = 16;
#[derive(Default)]
struct Observed {
    records: Vec<Record>,
    failed: Option<String>,
}
struct State {
    binding: Binding,
    observed: Mutex<Observed>,
    ready: Condvar,
}
impl State {
    fn retain(&self, record: Record) {
        // Source port is reported by the child after its connect attempt. Keep
        // only this listener/application's otherwise fully correlated drops.
        if record.source_port == 0
            || record.source_port == self.binding.listener_port
            || !self.binding.matches(
                &record,
                record.source_port,
                ticks(self.binding.start).saturating_add(30 * 10_000_000),
            )
        {
            return;
        }
        let mut observed = self.observed.lock().unwrap_or_else(|p| p.into_inner());
        if observed.records.len() == MAX_RECORDS {
            observed.failed = Some("WFP live observation exceeded its record bound".into());
        } else {
            observed.records.push(record);
        }
        self.ready.notify_all();
    }
    fn fail(&self, message: &str) {
        let mut observed = self.observed.lock().unwrap_or_else(|p| p.into_inner());
        if observed.failed.is_none() {
            observed.failed = Some(message.chars().take(256).collect());
        }
        self.ready.notify_all();
    }
}

unsafe extern "system" fn notification(context: *mut c_void, event: *const FWPM_NET_EVENT2) {
    if context.is_null() {
        return;
    }
    // The Box remains stable until unsubscribe has waited for all callbacks.
    let state = unsafe { &*context.cast::<State>() };
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<()> {
        ensure!(!event.is_null(), "WFP live event pointer is missing");
        if let Some(record) = Record::read(unsafe { &*event })? {
            state.retain(record);
        }
        Ok(())
    }));
    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => state.fail(&format!("{error:#}")),
        Err(_) => state.fail("WFP live event callback panicked"),
    }
}

pub(super) struct Subscription {
    engine: Engine,
    handle: HANDLE,
    state: Option<Box<State>>,
}
impl Subscription {
    pub(super) fn start(binding: Binding) -> Result<Self> {
        let mut subscription = Self {
            engine: Engine(null_mut()),
            handle: null_mut(),
            state: Some(Box::new(State {
                binding,
                observed: Mutex::new(Observed::default()),
                ready: Condvar::new(),
            })),
        };
        checked(
            unsafe {
                FwpmEngineOpen0(
                    null(),
                    RPC_C_AUTHN_WINNT,
                    null(),
                    null(),
                    &mut subscription.engine.0,
                )
            },
            "open read-only live observation",
        )?;
        let binding = subscription.binding();
        let mut app = FWP_BYTE_BLOB {
            size: binding.application_id.len() as u32,
            data: binding.application_id.as_ptr().cast_mut(),
        };
        let mut conditions = [
            FWPM_FILTER_CONDITION0 {
                fieldKey: FWPM_CONDITION_ALE_APP_ID,
                matchType: FWP_MATCH_EQUAL,
                conditionValue: FWP_CONDITION_VALUE0 {
                    r#type: FWP_BYTE_BLOB_TYPE,
                    Anonymous: FWP_CONDITION_VALUE0_0 { byteBlob: &mut app },
                },
            },
            FWPM_FILTER_CONDITION0 {
                fieldKey: FWPM_CONDITION_IP_REMOTE_PORT,
                matchType: FWP_MATCH_EQUAL,
                conditionValue: FWP_CONDITION_VALUE0 {
                    r#type: FWP_UINT16,
                    Anonymous: FWP_CONDITION_VALUE0_0 {
                        uint16: binding.listener_port,
                    },
                },
            },
        ];
        let mut template = FWPM_NET_EVENT_ENUM_TEMPLATE0 {
            startTime: binding.start,
            endTime: FILETIME {
                dwLowDateTime: ticks(binding.start).saturating_add(30 * 10_000_000) as u32,
                dwHighDateTime: (ticks(binding.start).saturating_add(30 * 10_000_000) >> 32) as u32,
            },
            numFilterConditions: conditions.len() as u32,
            filterCondition: conditions.as_mut_ptr(),
        };
        let request = FWPM_NET_EVENT_SUBSCRIPTION0 {
            enumTemplate: &mut template,
            ..Default::default()
        };
        let context = subscription.state() as *const State;
        checked(
            unsafe {
                FwpmNetEventSubscribe1(
                    subscription.engine.0,
                    &request,
                    Some(notification),
                    context.cast(),
                    &mut subscription.handle,
                )
            },
            "subscribe to live probe drops",
        )?;
        ensure!(
            !subscription.handle.is_null(),
            "WFP subscription handle is missing"
        );
        Ok(subscription)
    }
    fn state(&self) -> &State {
        self.state
            .as_deref()
            .expect("WFP observer state was removed")
    }
    pub(super) fn binding(&self) -> &Binding {
        &self.state().binding
    }
    pub(super) fn engine(&self) -> &Engine {
        &self.engine
    }
    pub(super) fn wait(&self, source_port: u16, end: FILETIME) -> Result<Record> {
        let state = self.state();
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut observed = state
            .observed
            .lock()
            .map_err(|_| anyhow::anyhow!("WFP live observer lock is poisoned"))?;
        loop {
            ensure!(
                observed.failed.is_none(),
                "WFP live observation failed: {}",
                observed.failed.as_deref().unwrap_or_default()
            );
            if let Some(record) = observed
                .records
                .iter()
                .find(|record| state.binding.matches(record, source_port, ticks(end)))
            {
                return Ok(record.clone());
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            ensure!(
                !remaining.is_zero(),
                "loopback timeout has no matching live AppContainerLoopback WFP drop (source={source_port},listener={})",
                state.binding.listener_port
            );
            observed = state
                .ready
                .wait_timeout(observed, remaining)
                .map_err(|_| anyhow::anyhow!("WFP live observer wait is poisoned"))?
                .0;
        }
    }
    pub(super) fn close(&mut self) -> Result<()> {
        if !self.handle.is_null() {
            // Never hold the callback mutex while waiting for unsubscribe.
            checked(
                unsafe { FwpmNetEventUnsubscribe0(self.engine.0, self.handle) },
                "unsubscribe live probe drops",
            )?;
            self.handle = null_mut();
        }
        Ok(())
    }
}
impl Drop for Subscription {
    fn drop(&mut self) {
        if self.close().is_err()
            && let Some(state) = self.state.take()
        {
            // A failed RPC cannot prove callbacks have drained. Retain this
            // bounded context until process exit rather than risk a use-after-
            // free; the engine session still closes through its existing guard.
            let _ = Box::leak(state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{capture, record};
    use super::*;

    fn state() -> State {
        State {
            binding: capture(),
            observed: Mutex::new(Observed::default()),
            ready: Condvar::new(),
        }
    }

    #[test]
    fn live_observation_rejects_other_resources_before_retaining_an_owned_drop() {
        let state = state();
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let state = &state;
                scope.spawn(move || {
                    for _ in 0..32 {
                        let mut unrelated = record();
                        unrelated.listener_port += 1;
                        state.retain(unrelated);
                    }
                });
            }
            state.retain(record());
        });
        let observed = state.observed.lock().unwrap();
        assert!(observed.failed.is_none());
        assert_eq!(observed.records.len(), 1);
        assert!(state.binding.matches(&observed.records[0], 45124, 200));
    }

    #[test]
    fn live_observation_overflow_and_callback_errors_fail_closed() {
        let state = state();
        for _ in 0..=MAX_RECORDS {
            state.retain(record());
        }
        let observed = state.observed.lock().unwrap();
        assert_eq!(observed.records.len(), MAX_RECORDS);
        assert!(observed.failed.as_deref().unwrap().contains("record bound"));
        drop(observed);
        let state = self::state();
        unsafe {
            notification((&state as *const State).cast_mut().cast(), null());
        }
        assert!(
            state
                .observed
                .lock()
                .unwrap()
                .failed
                .as_deref()
                .unwrap()
                .contains("pointer is missing")
        );
    }

    #[test]
    #[ignore = "requires a dedicated Windows VM administrator and actual read-only WFP subscription"]
    fn live_sdk_subscription_opens_and_unsubscribes_on_the_dedicated_vm() -> Result<()> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let binding = Binding::observe(&std::env::current_exe()?, listener.local_addr()?.port())?;
        let mut subscription = Subscription::start(binding)?;
        reachable(&listener)?;
        subscription.close()?;
        ensure!(
            subscription.handle.is_null(),
            "SDK subscription was not drained"
        );
        Ok(())
    }
}
