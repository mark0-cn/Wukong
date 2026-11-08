//! Read model credentials without mutating process-global environment or persisting secrets.
pub(crate) fn lookup(name: &str) -> Option<String> {
    if name.is_empty() || name.contains('\0') {
        return None;
    }
    let process = std::env::var(name).ok();
    resolve(process, || persistent(name))
}
fn resolve(process: Option<String>, fallback: impl FnOnce() -> Option<String>) -> Option<String> {
    process
        .filter(|value| !value.trim().is_empty())
        .or_else(fallback)
        .filter(|value| !value.trim().is_empty())
}
#[cfg(not(windows))]
fn persistent(_name: &str) -> Option<String> {
    None
}

#[cfg(windows)]
fn persistent(name: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegGetValueW,
    };
    let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    for (root, path) in [
        (HKEY_CURRENT_USER, "Environment"),
        (
            HKEY_LOCAL_MACHINE,
            "SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Environment",
        ),
    ] {
        let path: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
        let mut buffer = vec![0u16; 32768];
        let mut bytes = (buffer.len() * 2) as u32;
        // RegGetValueW only reads this named environment value. The bounded buffer
        // is aligned for UTF-16 and the API reports the number of bytes written.
        let result = unsafe {
            RegGetValueW(
                root,
                path.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ,
                std::ptr::null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut bytes,
            )
        };
        if result == 0 {
            let units = &buffer[..(bytes as usize / 2).min(buffer.len())];
            let end = units.iter().position(|c| *c == 0).unwrap_or(units.len());
            if let Ok(value) = String::from_utf16(&units[..end])
                && !value.trim().is_empty()
            {
                return Some(value);
            }
        }
    }
    None
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn process_credential_has_priority() {
        assert_eq!(
            resolve(Some("process-fixture".into()), || panic!(
                "must not read fallback"
            )),
            Some("process-fixture".into())
        );
    }
    #[test]
    fn missing_or_empty_process_credential_uses_persistent_value() {
        for process in [None, Some("  ".into())] {
            assert_eq!(
                resolve(process, || Some("persistent-fixture".into())),
                Some("persistent-fixture".into())
            );
        }
        assert_eq!(resolve(None, || Some(" ".into())), None);
        assert!(lookup("invalid\0name").is_none());
    }
}
