//! Bounded, read-only OS observations through an already owned process handle.
//! Windows owns mapping/policy interpretation. Never open paths supplied by the
//! target's loader lists or claim that a mapped pathname proves file contents.
use crate::isolation::security;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::{
    Foundation::{ERROR_INVALID_PARAMETER, FILETIME, HANDLE},
    System::{
        Memory::{
            MEM_COMMIT, MEM_IMAGE, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE, PAGE_EXECUTE_READ,
            PAGE_EXECUTE_READWRITE, PAGE_EXECUTE_WRITECOPY, PAGE_GUARD, PAGE_READONLY,
            PAGE_READWRITE, PAGE_WRITECOPY, VirtualQueryEx,
        },
        ProcessStatus::K32GetMappedFileNameW,
        SystemServices::{
            PROCESS_MITIGATION_ASLR_POLICY, PROCESS_MITIGATION_CONTROL_FLOW_GUARD_POLICY,
            PROCESS_MITIGATION_DEP_POLICY,
        },
        Threading::{
            GetProcessMitigationPolicy, GetProcessTimes, PROCESS_MITIGATION_POLICY,
            ProcessASLRPolicy, ProcessControlFlowGuardPolicy, ProcessDEPPolicy,
        },
    },
};

fn permissions(protection: u32) -> String {
    let basic = protection & 0xff;
    let read = matches!(
        basic,
        PAGE_READONLY
            | PAGE_READWRITE
            | PAGE_WRITECOPY
            | PAGE_EXECUTE_READ
            | PAGE_EXECUTE_READWRITE
            | PAGE_EXECUTE_WRITECOPY
    );
    let write = matches!(
        basic,
        PAGE_READWRITE | PAGE_WRITECOPY | PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY
    );
    let execute = matches!(
        basic,
        PAGE_EXECUTE | PAGE_EXECUTE_READ | PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY
    );
    [
        if read { 'r' } else { '-' },
        if write { 'w' } else { '-' },
        if execute { 'x' } else { '-' },
    ]
    .iter()
    .collect()
}

fn policy<T: Default>(
    process: HANDLE,
    kind: PROCESS_MITIGATION_POLICY,
    flags: impl FnOnce(&T) -> Value,
) -> Value {
    let mut value = T::default();
    if unsafe {
        GetProcessMitigationPolicy(process, kind, (&mut value as *mut T).cast(), size_of::<T>())
    } == 0
    {
        json!({"status":"unavailable","win32_error":std::io::Error::last_os_error().raw_os_error()})
    } else {
        json!({"status":"observed","raw":flags(&value)})
    }
}

fn mapped_name(process: HANDLE, address: *mut core::ffi::c_void) -> Value {
    let mut path = [0u16; 4096];
    let count =
        unsafe { K32GetMappedFileNameW(process, address, path.as_mut_ptr(), path.len() as u32) }
            as usize;
    if count == 0 {
        json!({"path":null,"win32_error":std::io::Error::last_os_error().raw_os_error()})
    } else if count >= path.len() {
        json!({"path":null,"error":"mapped path exceeds observation bound"})
    } else {
        json!({"path":String::from_utf16_lossy(&path[..count])})
    }
}

pub(crate) fn observe(process: HANDLE, pid: u32) -> Result<Value> {
    let mut created = FILETIME::default();
    let (mut exited, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    security::check(unsafe {
        GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user)
    })?;
    let started = Instant::now();
    let mut address = 0usize;
    let mut maps = Vec::new();
    let mut modules = Vec::new();
    let mut images = BTreeSet::new();
    let mut complete = false;
    let mut query_error = None;
    for _ in 0..16384 {
        if maps.len() == 4096 || started.elapsed() >= Duration::from_secs(2) {
            break;
        }
        let mut region = MEMORY_BASIC_INFORMATION::default();
        let read = unsafe {
            VirtualQueryEx(
                process,
                address as *const _,
                &mut region,
                size_of_val(&region),
            )
        };
        if read == 0 {
            let error = std::io::Error::last_os_error().raw_os_error();
            complete = error == Some(ERROR_INVALID_PARAMETER as i32) && address > 0;
            if !complete {
                query_error = error;
            }
            break;
        }
        ensure!(
            read == size_of_val(&region),
            "incomplete Windows memory-region observation"
        );
        let end = (region.BaseAddress as usize)
            .checked_add(region.RegionSize)
            .context("Windows memory-region address overflow")?;
        ensure!(end > address, "Windows memory-region query did not advance");
        if region.State == MEM_COMMIT {
            maps.push(json!({"start":region.BaseAddress as usize,"end":end,
                "allocation_base":region.AllocationBase as usize,
                "permissions":permissions(region.Protect),"protection":region.Protect,
                "guard":region.Protect & PAGE_GUARD != 0,"type":region.Type}));
            if region.Type == MEM_IMAGE
                && images.insert(region.AllocationBase as usize)
                && modules.len() < 64
            {
                modules.push(json!({"base":region.AllocationBase as usize,
                    "mapped_file":mapped_name(process, region.BaseAddress),"sha256":null,
                    "identity_status":"path_observed_contents_unverified"}));
            }
        }
        address = end;
    }
    Ok(json!({"pid":pid,"status":"observed","platform":"windows",
        "created_filetime":(u64::from(created.dwHighDateTime)<<32) | u64::from(created.dwLowDateTime),
        "observed_unix_ms":SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis(),
        "maps":maps,"modules":modules,"complete_maps":complete,
        "complete_modules":complete && images.len() <= 64,"query_win32_error":query_error,
        "protections":{
            "dep":policy::<PROCESS_MITIGATION_DEP_POLICY>(process, ProcessDEPPolicy, |p| json!({"flags":unsafe {p.Anonymous.Flags},"permanent":p.Permanent})),
            "aslr":policy::<PROCESS_MITIGATION_ASLR_POLICY>(process, ProcessASLRPolicy, |p| json!({"flags":unsafe {p.Anonymous.Flags}})),
            "cfg":policy::<PROCESS_MITIGATION_CONTROL_FLOW_GUARD_POLICY>(process, ProcessControlFlowGuardPolicy, |p| json!({"flags":unsafe {p.Anonymous.Flags}}))},
        "limits":["Sequential live observations, not an atomic or suspended snapshot.",
            "Only committed regions are listed. Guard flags qualify page permissions.",
            "Image mappings can contain modified copy-on-write pages; paths do not prove content identity.",
            "An unavailable policy is unknown, not disabled."]}))
}

pub(crate) fn preview(mut value: Value) -> Value {
    for (name, limit) in [("maps", 64), ("modules", 16)] {
        if let Some(items) = value[name].as_array_mut() {
            let total = items.len();
            items.truncate(limit);
            if name == "modules" {
                for item in items {
                    if let Some(path) = item["mapped_file"]["path"].as_str() {
                        let shortened: String = path.chars().take(256).collect();
                        if shortened != path {
                            item["mapped_file"]["path"] = json!(shortened);
                            item["mapped_file"]["path_truncated"] = json!(true);
                        }
                    }
                }
            }
            value[format!("{name}_observed")] = json!(total);
            value[format!("{name}_preview_truncated")] = json!(total > limit);
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    #[test]
    fn observes_real_committed_memory_and_policies_without_reading_files() -> Result<()> {
        let allocation = vec![0x61u8; 8192];
        let address = allocation.as_ptr() as u64;
        let observed = observe(unsafe { GetCurrentProcess() }, std::process::id())?;
        assert_eq!(observed["pid"], std::process::id());
        assert!(observed["created_filetime"].as_u64().unwrap() > 0);
        let mapping = observed["maps"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| {
                m["start"].as_u64().unwrap() <= address && address < m["end"].as_u64().unwrap()
            })
            .unwrap();
        assert_eq!(mapping["permissions"], "rw-");
        assert!(!observed["modules"].as_array().unwrap().is_empty());
        assert!(
            observed["modules"]
                .as_array()
                .unwrap()
                .iter()
                .all(|m| m["sha256"].is_null())
        );
        assert_eq!(observed["protections"]["aslr"]["status"], "observed");
        assert!(serde_json::to_vec(&preview(observed))?.len() < 60 * 1024);
        Ok(())
    }
    #[test]
    fn preview_bounds_unicode_paths_and_preserves_original() -> Result<()> {
        let original = json!({"maps":vec![json!({"start":0,"end":1});4096],
            "modules":vec![json!({"mapped_file":{"path":"中文".repeat(4095)},"sha256":null});64]});
        let bounded = preview(original.clone());
        assert_eq!(bounded["maps_observed"], 4096);
        assert_eq!(bounded["maps_preview_truncated"], true);
        assert_eq!(bounded["modules_preview_truncated"], true);
        assert_eq!(bounded["modules"][0]["mapped_file"]["path_truncated"], true);
        assert_eq!(
            original["modules"][0]["mapped_file"]["path"]
                .as_str()
                .unwrap()
                .chars()
                .count(),
            8190
        );
        assert!(serde_json::to_vec(&bounded)?.len() < 60 * 1024);
        Ok(())
    }
}
