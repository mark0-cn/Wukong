//! Fixed loopback qualification. A timeout needs an actual WFP drop for this
//! probe's socket tuple; it is never a substitute for observed enforcement.
use crate::security;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    io::ErrorKind,
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::Path,
    ptr::{null, null_mut},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{FILETIME, HANDLE},
    NetworkManagement::WindowsFilteringPlatform::*,
    System::{Rpc::RPC_C_AUTHN_WINNT, SystemInformation::GetSystemTimeAsFileTime},
};

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Observation {
    outcome: Outcome,
    source_port: u16,
    os_error: Option<i32>,
}
#[derive(Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Outcome {
    PermissionDenied,
    TimedOut,
}

pub(super) fn observe(listener_port: u16) -> Result<Observation> {
    let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
    socket.bind(&SocketAddr::from((Ipv4Addr::LOCALHOST, 0)).into())?;
    let source_port = socket
        .local_addr()?
        .as_socket()
        .context("missing probe socket address")?
        .port();
    match socket.connect_timeout(
        &SocketAddr::from((Ipv4Addr::LOCALHOST, listener_port)).into(),
        Duration::from_secs(2),
    ) {
        Err(error) => {
            let outcome = match error.kind() {
                ErrorKind::PermissionDenied => Outcome::PermissionDenied,
                ErrorKind::TimedOut => Outcome::TimedOut,
                _ => anyhow::bail!(
                    "inconclusive loopback error (kind={:?}, os={:?}): {error}",
                    error.kind(),
                    error.raw_os_error()
                ),
            };
            Ok(Observation {
                outcome,
                source_port,
                os_error: error.raw_os_error(),
            })
        }
        Ok(()) => {
            anyhow::bail!("AppContainer unexpectedly connected to the owned loopback listener")
        }
    }
}

/// Check this exact listening socket, including the accepted peer. An extra
/// connection cannot be mistaken for the System control or silently discarded.
pub(super) fn reachable(listener: &TcpListener) -> Result<()> {
    let stream = TcpStream::connect_timeout(&listener.local_addr()?, Duration::from_secs(2))?;
    let peer = stream.local_addr()?;
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        match listener.accept() {
            Ok((accepted, actual)) => {
                ensure!(
                    actual == peer,
                    "unexpected connection reached the probe listener"
                );
                drop(accepted);
                break;
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error).context("accept owned loopback control"),
        }
    }
    match listener.accept() {
        Err(error) if error.kind() == ErrorKind::WouldBlock => Ok(()),
        _ => anyhow::bail!("probe listener has an unexpected connection or accept failure"),
    }
}

struct Memory<T>(*mut T);
impl<T> Drop for Memory<T> {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                FwpmFreeMemory0((&mut self.0 as *mut *mut T).cast());
            }
        }
    }
}
struct Engine(HANDLE);
impl Drop for Engine {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                FwpmEngineClose0(self.0);
            }
        }
    }
}
struct Enumeration<'a> {
    engine: &'a Engine,
    handle: HANDLE,
}
impl Drop for Enumeration<'_> {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                FwpmNetEventDestroyEnumHandle0(self.engine.0, self.handle);
            }
        }
    }
}
fn checked(status: u32, operation: &str) -> Result<()> {
    ensure!(status == 0, "WFP {operation} failed: 0x{status:08x}");
    Ok(())
}
fn time() -> FILETIME {
    let mut value = FILETIME::default();
    unsafe {
        GetSystemTimeAsFileTime(&mut value);
    }
    value
}
fn ticks(time: FILETIME) -> u64 {
    (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
}
fn blob(value: &FWP_BYTE_BLOB) -> Result<&[u8]> {
    ensure!(
        value.size > 0 && value.size <= 65536 && !value.data.is_null(),
        "invalid WFP application identifier"
    );
    Ok(unsafe { std::slice::from_raw_parts(value.data, value.size as usize) })
}
fn guid_equal(a: windows_sys::core::GUID, b: windows_sys::core::GUID) -> bool {
    a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
}
fn filter_name(pointer: *const u16) -> Result<String> {
    ensure!(!pointer.is_null(), "WFP filter has no name");
    let mut name = Vec::new();
    for offset in 0..128 {
        let value = unsafe { *pointer.add(offset) };
        if value == 0 {
            return String::from_utf16(&name).context("invalid WFP filter name");
        }
        name.push(value);
    }
    anyhow::bail!("WFP filter name exceeds qualification bound")
}

pub(super) struct Capture {
    start: FILETIME,
    application_id: Vec<u8>,
    listener_port: u16,
}
impl Capture {
    pub(super) fn start(executable: &Path, listener_port: u16) -> Result<Self> {
        security::require_system()?;
        let path = security::wide(executable.as_os_str())?;
        let mut app = Memory(null_mut());
        checked(
            unsafe { FwpmGetAppIdFromFileName0(path.as_ptr(), &mut app.0) },
            "application identity",
        )?;
        ensure!(!app.0.is_null(), "WFP application identity is missing");
        let application_id = blob(unsafe { &*app.0 })?.to_vec();
        Ok(Self {
            start: time(),
            application_id,
            listener_port,
        })
    }
    pub(super) fn verify(&self, observation: &Observation) -> Result<Value> {
        ensure!(
            observation.source_port > 0 && observation.source_port != self.listener_port,
            "invalid probe source port"
        );
        if observation.outcome == Outcome::PermissionDenied {
            ensure!(
                matches!(observation.os_error, Some(5 | 10013)),
                "network permission denial has no matching Windows error"
            );
            return Ok(
                json!({"method":"socket_permission_denied","os_error":observation.os_error,"source_port":observation.source_port,"listener_port":self.listener_port}),
            );
        }
        let mut engine = Engine(null_mut());
        checked(
            unsafe { FwpmEngineOpen0(null(), RPC_C_AUTHN_WINNT, null(), null(), &mut engine.0) },
            "open read-only observation",
        )?;
        let end = time();
        ensure!(
            ticks(end) >= ticks(self.start) && ticks(end) - ticks(self.start) <= 30 * 10_000_000,
            "probe WFP time window is invalid"
        );
        // An enumeration is a snapshot. Recreate it for bounded late delivery,
        // preserving the original event-time window. Never enable collection.
        for attempt in 0..5 {
            if let Some(evidence) = self.find(&engine, observation.source_port, end)? {
                return Ok(evidence);
            }
            if attempt < 4 {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        anyhow::bail!(
            "loopback timeout has no matching AppContainerLoopback WFP drop (source={},listener={})",
            observation.source_port,
            self.listener_port
        )
    }
    fn find(&self, engine: &Engine, source_port: u16, end: FILETIME) -> Result<Option<Value>> {
        let mut app = FWP_BYTE_BLOB {
            size: self.application_id.len() as u32,
            data: self.application_id.as_ptr().cast_mut(),
        };
        let mut condition = FWPM_FILTER_CONDITION0 {
            fieldKey: FWPM_CONDITION_ALE_APP_ID,
            matchType: FWP_MATCH_EQUAL,
            conditionValue: FWP_CONDITION_VALUE0 {
                r#type: FWP_BYTE_BLOB_TYPE,
                Anonymous: FWP_CONDITION_VALUE0_0 { byteBlob: &mut app },
            },
        };
        let template = FWPM_NET_EVENT_ENUM_TEMPLATE0 {
            startTime: self.start,
            endTime: end,
            numFilterConditions: 1,
            filterCondition: &mut condition,
        };
        let mut enumeration = Enumeration {
            engine,
            handle: null_mut(),
        };
        checked(
            unsafe { FwpmNetEventCreateEnumHandle0(engine.0, &template, &mut enumeration.handle) },
            "create event enumeration",
        )?;
        // At most 1024 same-application events; other applications' data never
        // enters a report. The actual tuple/time/SID is checked independently.
        for _ in 0..16 {
            let mut entries = Memory::<*mut FWPM_NET_EVENT2>(null_mut());
            let mut count = 0;
            checked(
                unsafe {
                    FwpmNetEventEnum2(engine.0, enumeration.handle, 64, &mut entries.0, &mut count)
                },
                "enumerate events",
            )?;
            ensure!(
                count <= 64 && (count == 0 || !entries.0.is_null()),
                "invalid WFP event page"
            );
            if count == 0 {
                return Ok(None);
            }
            for pointer in unsafe { std::slice::from_raw_parts(entries.0, count as usize) } {
                ensure!(!pointer.is_null(), "WFP event is missing");
                let event = unsafe { &**pointer };
                if let Some(record) = Record::read(event)?
                    && self.matches(&record, source_port, ticks(end))
                {
                    self.verify_filter(engine, record.filter_id, record.layer_id)?;
                    return Ok(Some(
                        json!({"method":"wfp_appcontainer_loopback_drop","filter_id":record.filter_id,"layer_id":record.layer_id,"event_time":record.time,"window_start":ticks(self.start),"window_end":ticks(end),"source_port":source_port,"listener_port":self.listener_port,"listener_sid":"S-1-5-18","application_id_matched":true,"loopback":true,"filter":"AppContainerLoopback","action":"block"}),
                    ));
                }
            }
            if count < 64 {
                return Ok(None);
            }
        }
        anyhow::bail!("WFP probe event enumeration exceeds bound")
    }
    fn matches(&self, record: &Record, source_port: u16, end: u64) -> bool {
        record.time >= ticks(self.start)
            && record.time <= end
            && record.application_id == self.application_id
            && record.listener_port == self.listener_port
            && record.source_port == source_port
            && record.local_address == 0x7f000001
            && record.remote_address == 0x7f000001
            && record.user_sid == "S-1-5-18"
            && record.loopback
            // msFwpDirection uses FWP_DIRECTION_IN (0x3900), not the
            // differently encoded FWP_DIRECTION_INBOUND (1).
            && record.direction == 0x3900
    }
    fn verify_filter(&self, engine: &Engine, id: u64, layer_id: u16) -> Result<()> {
        let mut filter = Memory(null_mut());
        checked(
            unsafe { FwpmFilterGetById0(engine.0, id, &mut filter.0) },
            "read enforcing filter",
        )?;
        ensure!(!filter.0.is_null(), "WFP enforcing filter is missing");
        let filter = unsafe { &*filter.0 };
        ensure!(
            filter.filterId == id
                && filter.action.r#type == FWP_ACTION_BLOCK
                && guid_equal(filter.layerKey, FWPM_LAYER_ALE_AUTH_RECV_ACCEPT_V4)
                && filter_name(filter.displayData.name)? == "AppContainerLoopback",
            "WFP drop is not the AppContainer loopback block filter"
        );
        let mut layer = Memory(null_mut());
        checked(
            unsafe { FwpmLayerGetById0(engine.0, layer_id, &mut layer.0) },
            "read drop layer",
        )?;
        ensure!(
            !layer.0.is_null() && guid_equal(unsafe { (*layer.0).layerKey }, filter.layerKey),
            "WFP drop layer differs from enforcing filter"
        );
        Ok(())
    }
}

struct Record {
    time: u64,
    application_id: Vec<u8>,
    listener_port: u16,
    source_port: u16,
    local_address: u32,
    remote_address: u32,
    user_sid: String,
    loopback: bool,
    direction: u32,
    filter_id: u64,
    layer_id: u16,
}
impl Record {
    fn read(event: &FWPM_NET_EVENT2) -> Result<Option<Self>> {
        let header = &event.header;
        let required = FWPM_NET_EVENT_FLAG_APP_ID_SET
            | FWPM_NET_EVENT_FLAG_USER_ID_SET
            | FWPM_NET_EVENT_FLAG_IP_VERSION_SET
            | FWPM_NET_EVENT_FLAG_IP_PROTOCOL_SET
            | FWPM_NET_EVENT_FLAG_LOCAL_ADDR_SET
            | FWPM_NET_EVENT_FLAG_REMOTE_ADDR_SET
            | FWPM_NET_EVENT_FLAG_LOCAL_PORT_SET
            | FWPM_NET_EVENT_FLAG_REMOTE_PORT_SET;
        if event.r#type != FWPM_NET_EVENT_TYPE_CLASSIFY_DROP
            || header.flags & required != required
            || header.ipVersion != FWP_IP_VERSION_V4
            || header.ipProtocol != 6
        {
            return Ok(None);
        }
        let drop = unsafe { event.Anonymous.classifyDrop };
        ensure!(
            !drop.is_null() && !header.userId.is_null(),
            "WFP drop identity is missing"
        );
        let drop = unsafe { &*drop };
        Ok(Some(Self {
            time: ticks(header.timeStamp),
            application_id: blob(&header.appId)?.to_vec(),
            listener_port: header.localPort,
            source_port: header.remotePort,
            local_address: unsafe { header.Anonymous1.localAddrV4 },
            remote_address: unsafe { header.Anonymous2.remoteAddrV4 },
            user_sid: security::sid_text(header.userId.cast())?,
            loopback: drop.isLoopback != 0,
            direction: drop.msFwpDirection,
            filter_id: drop.filterId,
            layer_id: drop.layerId,
        }))
    }
}

pub(super) fn validate_evidence(value: &Value) -> Result<()> {
    let source = value["source_port"]
        .as_u64()
        .context("missing network source port")?;
    let listener = value["listener_port"]
        .as_u64()
        .context("missing network listener port")?;
    ensure!(
        (1..=65535).contains(&source) && (1..=65535).contains(&listener) && source != listener,
        "invalid network evidence ports"
    );
    match value["method"].as_str() {
        Some("socket_permission_denied") => ensure!(
            matches!(value["os_error"].as_i64(), Some(5 | 10013)),
            "network denial error differs"
        ),
        Some("wfp_appcontainer_loopback_drop") => {
            let (start, end, event) = (
                value["window_start"].as_u64(),
                value["window_end"].as_u64(),
                value["event_time"].as_u64(),
            );
            ensure!(
                matches!((start,end,event),(Some(a),Some(b),Some(c)) if b>=a && b-a<=30*10_000_000 && c>=a && c<=b)
                    && value["filter_id"].as_u64().is_some_and(|id| id > 0)
                    && value["layer_id"]
                        .as_u64()
                        .is_some_and(|id| id > 0 && id <= 65535)
                    && value["listener_sid"] == "S-1-5-18"
                    && value["application_id_matched"] == true
                    && value["loopback"] == true
                    && value["filter"] == "AppContainerLoopback"
                    && value["action"] == "block",
                "WFP denial evidence is incomplete"
            );
        }
        _ => anyhow::bail!("unknown network qualification evidence"),
    }
    Ok(())
}

#[cfg(test)]
#[path = "resource_network_tests.rs"]
mod tests;
