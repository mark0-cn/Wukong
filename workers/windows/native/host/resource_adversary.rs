//! Fixed startup probe: change only this contained process/thread's DACL, then
//! request (never exercise) dangerous rights to the still-live System host.
use crate::{
    isolation::security::{self, Handle, SecurityDescriptor},
    isolation::token_policy,
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, ptr::null_mut};
use windows_sys::Win32::{
    Foundation::{ERROR_ACCESS_DENIED, HANDLE},
    Security::{
        Authorization::{SE_KERNEL_OBJECT, SetSecurityInfo},
        DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, PROTECTED_DACL_SECURITY_INFORMATION,
        TOKEN_QUERY,
    },
    Storage::FileSystem::{WRITE_DAC, WRITE_OWNER},
    System::Threading::*,
};

const PROCESS_RIGHTS: &[(&str, u32)] = &[
    ("read_memory", PROCESS_VM_READ),
    ("write_memory", PROCESS_VM_WRITE),
    ("vm_operation", PROCESS_VM_OPERATION),
    ("duplicate_handle", PROCESS_DUP_HANDLE),
    ("create_thread", PROCESS_CREATE_THREAD),
    ("create_process", PROCESS_CREATE_PROCESS),
    ("set_information", PROCESS_SET_INFORMATION),
    ("terminate", PROCESS_TERMINATE),
    ("write_dacl", WRITE_DAC),
    ("write_owner", WRITE_OWNER),
];
const THREAD_RIGHTS: &[(&str, u32)] = &[
    ("get_context", THREAD_GET_CONTEXT),
    ("set_context", THREAD_SET_CONTEXT),
    ("impersonate", THREAD_IMPERSONATE),
    ("direct_impersonation", THREAD_DIRECT_IMPERSONATION),
    ("set_token", THREAD_SET_THREAD_TOKEN),
    ("suspend_resume", THREAD_SUSPEND_RESUME),
    ("terminate", THREAD_TERMINATE),
    ("write_dacl", WRITE_DAC),
    ("write_owner", WRITE_OWNER),
];

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Denial {
    right: String,
    mask: u32,
    error: u32,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Observation {
    source: String,
    self_process_dacl_replaced: bool,
    self_thread_dacl_replaced: bool,
    protected_process_id: u32,
    protected_thread_id: u32,
    process_denials: Vec<Denial>,
    thread_denials: Vec<Denial>,
}

fn replace_own_dacl(handle: HANDLE, access: u32) -> Result<()> {
    let descriptor = SecurityDescriptor::from_sddl(&format!("D:P(A;;0x{access:08x};;;WD)"))?;
    let (mut present, mut defaulted, mut acl) = (0, 0, null_mut());
    security::check(unsafe {
        GetSecurityDescriptorDacl(
            descriptor.attributes().lpSecurityDescriptor,
            &mut present,
            &mut acl,
            &mut defaulted,
        )
    })?;
    ensure!(
        present != 0 && !acl.is_null(),
        "missing fixed self-DACL probe"
    );
    let status = unsafe {
        SetSecurityInfo(
            handle,
            SE_KERNEL_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            acl,
            null_mut(),
        )
    };
    ensure!(
        status == 0,
        "self pseudo-handle DACL mutation failed: {status}"
    );
    // Read back the actual kernel object. Do not infer mutation from a schema,
    // a successful API return, or a changed user-mode descriptor buffer.
    security::verify_descriptor(
        SecurityDescriptor::from_object(handle, SE_KERNEL_OBJECT)?
            .attributes()
            .lpSecurityDescriptor,
        "S-1-5-18",
        &BTreeMap::from([("S-1-1-0".into(), access)]),
        true,
    )
}

pub(super) fn check(process: u32, thread: u32) -> Result<Value> {
    let mut token = null_mut();
    security::check(unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) })?;
    let token = Handle(token);
    token_policy::verify_network_isolation(token.0)?;
    crate::isolation::processes::verify_sandbox_privileges(&token)?;
    ensure!(
        process > 0
            && thread > 0
            && process != unsafe { GetCurrentProcessId() }
            && thread != unsafe { GetCurrentThreadId() },
        "invalid fixed System host identity"
    );
    replace_own_dacl(unsafe { GetCurrentProcess() }, PROCESS_ALL_ACCESS)?;
    replace_own_dacl(unsafe { GetCurrentThread() }, THREAD_ALL_ACCESS)?;
    let mut result = Observation {
        source: "sdk_self_dacl_and_open_rights".into(),
        self_process_dacl_replaced: true,
        self_thread_dacl_replaced: true,
        protected_process_id: process,
        protected_thread_id: thread,
        process_denials: Vec::new(),
        thread_denials: Vec::new(),
    };
    for (rights, process_object) in [(PROCESS_RIGHTS, true), (THREAD_RIGHTS, false)] {
        for &(name, mask) in rights {
            let handle = unsafe {
                if process_object {
                    OpenProcess(mask, 0, process)
                } else {
                    OpenThread(mask, 0, thread)
                }
            };
            let error = std::io::Error::last_os_error();
            let held = Handle(handle);
            ensure!(
                held.0.is_null() && error.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32),
                "contained self-DACL probe obtained {name} or denial was inconclusive: {error}"
            );
            let denial = Denial {
                right: name.into(),
                mask,
                error: ERROR_ACCESS_DENIED,
            };
            if process_object {
                result.process_denials.push(denial);
            } else {
                result.thread_denials.push(denial);
            }
        }
    }
    let value = serde_json::to_value(result)?;
    validate(&value)?;
    Ok(value)
}

pub(super) fn validate(value: &Value) -> Result<()> {
    let value: Observation = serde_json::from_value(value.clone())?;
    ensure!(
        value.source == "sdk_self_dacl_and_open_rights"
            && value.self_process_dacl_replaced
            && value.self_thread_dacl_replaced
            && value.protected_process_id > 0
            && value.protected_thread_id > 0,
        "missing self-DACL adversarial observation"
    );
    for (actual, expected) in [
        (&value.process_denials, PROCESS_RIGHTS),
        (&value.thread_denials, THREAD_RIGHTS),
    ] {
        ensure!(
            actual.len() == expected.len(),
            "incomplete adversarial rights coverage"
        );
        for (actual, &(right, mask)) in actual.iter().zip(expected) {
            ensure!(
                actual.right == right && actual.mask == mask && actual.error == ERROR_ACCESS_DENIED,
                "adversarial denial differs from the required SDK right/error"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ordinary_tokens_cannot_mutate_their_dacl_through_the_probe() -> Result<()> {
        let error = check(1, 1).unwrap_err();
        assert!(error.to_string().contains("AppContainer"));
        Ok(())
    }
    #[test]
    fn schema_only_partial_and_wrong_rights_never_qualify_denials() -> Result<()> {
        let make = |rights: &[(&str, u32)]| {
            rights
                .iter()
                .map(|&(right, mask)| Denial {
                    right: right.into(),
                    mask,
                    error: ERROR_ACCESS_DENIED,
                })
                .collect()
        };
        let observation = Observation {
            source: "sdk_self_dacl_and_open_rights".into(),
            self_process_dacl_replaced: true,
            self_thread_dacl_replaced: true,
            protected_process_id: 100,
            protected_thread_id: 101,
            process_denials: make(PROCESS_RIGHTS),
            thread_denials: make(THREAD_RIGHTS),
        };
        let original = serde_json::to_value(observation)?;
        validate(&original)?;
        for (field, changed) in [
            ("self_process_dacl_replaced", serde_json::json!(false)),
            ("self_thread_dacl_replaced", serde_json::json!(false)),
            ("protected_process_id", serde_json::json!(0)),
            ("process_denials", serde_json::json!([])),
        ] {
            let mut value = original.clone();
            value[field] = changed;
            assert!(validate(&value).is_err());
        }
        for field in ["process_denials", "thread_denials"] {
            for (key, changed) in [
                ("mask", serde_json::json!(0)),
                ("error", serde_json::json!(87)),
                ("right", serde_json::json!("wrong")),
            ] {
                let mut value = original.clone();
                value[field][0][key] = changed;
                assert!(validate(&value).is_err());
            }
        }
        Ok(())
    }
}
