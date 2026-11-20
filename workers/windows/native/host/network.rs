//! Best-effort guest IPv4 enumeration for the exported installation receipt.
//! Address candidates are public connection hints, never trust or isolation proof.
use anyhow::{Result, ensure};
use std::net::Ipv4Addr;

/// Maximum number of guest addresses retained in `connection.json`.
use wukong_worker_package::MAX_CANDIDATE_ADDRESSES;
const MAXIMUM_TABLE_BYTES: u32 = 4 * 1024 * 1024;
const MAXIMUM_ADAPTERS: usize = 4096;
const MAXIMUM_LIST_ENTRIES: usize = 4096;

/// Injectable source of this guest's local IPv4 addresses in platform order.
/// Tests inject fixed lists; only [`SystemAdapters`] calls the network stack.
pub(crate) trait AdapterSource {
    fn ipv4_addresses(&self) -> Result<Vec<Ipv4Addr>>;
}

/// Enumerate the guest's IPv4 addresses with `GetAdaptersInfo`. The installer
/// receipt needs IPv4 only, so the simpler legacy table avoids IPv6 aggregates.
pub(crate) struct SystemAdapters;

impl AdapterSource for SystemAdapters {
    fn ipv4_addresses(&self) -> Result<Vec<Ipv4Addr>> {
        use std::ffi::CStr;
        use std::ptr::null_mut;
        use windows_sys::Win32::{
            Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_NO_DATA},
            NetworkManagement::IpHelper::{GetAdaptersInfo, IP_ADAPTER_INFO},
        };
        let mut size = 0;
        // SAFETY: A null table with a valid out-param is the documented size probe; the call
        // writes only the bounded required-size value into `size`.
        let status = unsafe { GetAdaptersInfo(null_mut(), &mut size) };
        if status == 0 || status == ERROR_NO_DATA {
            return Ok(Vec::new());
        }
        ensure!(
            status == ERROR_BUFFER_OVERFLOW,
            "cannot size the guest adapter table (Windows error {status})"
        );
        ensure!(
            size > 0 && size <= MAXIMUM_TABLE_BYTES,
            "guest adapter table exceeds its inspection limit"
        );
        // `u64` slots keep the buffer aligned for the pointer-bearing adapter records.
        let mut table = vec![0u64; (size as usize).div_ceil(std::mem::size_of::<u64>())];
        // SAFETY: `table` is pointer-aligned and spans at least `size` bytes; the SDK writes
        // only `size` bytes of records into it and updates `size`, and the buffer outlives
        // the adapter traversal below.
        let status =
            unsafe { GetAdaptersInfo(table.as_mut_ptr().cast::<IP_ADAPTER_INFO>(), &mut size) };
        ensure!(
            status == 0,
            "cannot read the guest adapter table (Windows error {status})"
        );
        let mut addresses = Vec::new();
        // A successful call fills `size` bytes of adapter records starting at the buffer base,
        // and every `Next` link stays inside that SDK-filled table.
        let mut adapter = table.as_ptr().cast::<IP_ADAPTER_INFO>();
        let mut adapters = 0;
        while !adapter.is_null() {
            adapters += 1;
            ensure!(
                adapters <= MAXIMUM_ADAPTERS,
                "guest adapter table exceeds its entry limit"
            );
            // SAFETY: `adapter` is a non-null record inside the SDK-filled table, so its inline
            // `IpAddressList` field is a valid first list node.
            let mut entry = unsafe { std::ptr::addr_of!((*adapter).IpAddressList) };
            let mut entries = 0;
            while !entry.is_null() {
                entries += 1;
                ensure!(
                    entries <= MAXIMUM_LIST_ENTRIES,
                    "guest adapter address list exceeds its entry limit"
                );
                // SAFETY: `entry` points inside the same SDK-filled table and
                // `IpAddress.String` is a fixed NUL-terminated ANSI buffer owned by it.
                let text = unsafe { CStr::from_ptr((*entry).IpAddress.String.as_ptr()) };
                if let Some(address) = text
                    .to_str()
                    .ok()
                    .and_then(|text| text.parse::<Ipv4Addr>().ok())
                {
                    addresses.push(address);
                }
                // SAFETY: `Next` links only inside the SDK-filled table and is null at the end.
                entry = unsafe { (*entry).Next };
            }
            // SAFETY: `Next` links only inside the SDK-filled table and is null at the end.
            adapter = unsafe { (*adapter).Next };
        }
        Ok(addresses)
    }
}

/// Receipt candidates are best effort: an adapter enumeration failure yields no
/// candidates and never fails an otherwise successful installation.
pub(crate) fn receipt_candidates(source: &impl AdapterSource) -> Vec<String> {
    source
        .ipv4_addresses()
        .map(|addresses| candidate_addresses(&addresses))
        .unwrap_or_default()
}

/// Deterministic candidate projection of enumerable adapters: drop loopback,
/// unspecified, multicast and broadcast addresses, order RFC 1918 private ranges
/// first and then by ascending numeric value, remove duplicates and cap the list.
pub(crate) fn candidate_addresses(addresses: &[Ipv4Addr]) -> Vec<String> {
    let mut candidates: Vec<Ipv4Addr> = addresses
        .iter()
        .copied()
        .filter(|address| {
            !address.is_loopback()
                && !address.is_unspecified()
                && !address.is_multicast()
                && !address.is_broadcast()
        })
        .collect();
    candidates.sort_by_key(|address| (!address.is_private(), u32::from(*address)));
    candidates.dedup();
    candidates.truncate(MAX_CANDIDATE_ADDRESSES);
    candidates
        .into_iter()
        .map(|address| address.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{AdapterSource, MAX_CANDIDATE_ADDRESSES, candidate_addresses, receipt_candidates};
    use anyhow::Result;
    use std::net::Ipv4Addr;

    struct FixedAdapters(Vec<Ipv4Addr>);

    impl AdapterSource for FixedAdapters {
        fn ipv4_addresses(&self) -> Result<Vec<Ipv4Addr>> {
            Ok(self.0.clone())
        }
    }

    struct UnavailableAdapters;

    impl AdapterSource for UnavailableAdapters {
        fn ipv4_addresses(&self) -> Result<Vec<Ipv4Addr>> {
            anyhow::bail!("adapter enumeration unavailable")
        }
    }

    fn addresses(values: &[&str]) -> Vec<Ipv4Addr> {
        values
            .iter()
            .map(|value| value.parse().expect("fixture address"))
            .collect()
    }

    #[test]
    fn candidates_are_private_first_and_sorted() {
        let candidates = candidate_addresses(&addresses(&[
            "192.168.1.20",
            "8.8.8.8",
            "10.0.0.2",
            "172.16.5.4",
            "172.32.0.1",
            "10.0.0.2",
        ]));
        assert_eq!(
            candidates,
            [
                "10.0.0.2",
                "172.16.5.4",
                "192.168.1.20",
                "8.8.8.8",
                "172.32.0.1"
            ]
        );
    }

    #[test]
    fn candidates_exclude_non_unicast_and_duplicate_addresses() {
        let candidates = candidate_addresses(&addresses(&[
            "127.0.0.1",
            "0.0.0.0",
            "224.0.0.1",
            "255.255.255.255",
            "169.254.10.20",
            "10.0.0.2",
            "10.0.0.2",
        ]));
        assert_eq!(candidates, ["10.0.0.2", "169.254.10.20"]);
    }

    #[test]
    fn candidates_are_capped() {
        let mut values = (1..=12)
            .map(|last| Ipv4Addr::new(192, 168, 0, last))
            .collect::<Vec<_>>();
        values.push(Ipv4Addr::new(203, 0, 113, 5));
        let candidates = candidate_addresses(&values);
        assert_eq!(candidates.len(), MAX_CANDIDATE_ADDRESSES);
        assert_eq!(candidates.first().map(String::as_str), Some("192.168.0.1"));
        assert_eq!(candidates.last().map(String::as_str), Some("192.168.0.8"));
        assert!(!candidates.iter().any(|value| value == "203.0.113.5"));
    }

    #[test]
    fn receipt_candidates_use_the_injected_adapter_source() {
        let source = FixedAdapters(addresses(&["127.0.0.1", "10.1.2.3"]));
        assert_eq!(receipt_candidates(&source), ["10.1.2.3"]);
    }

    #[test]
    fn receipt_candidates_degrade_to_an_empty_list() {
        assert!(receipt_candidates(&UnavailableAdapters).is_empty());
    }
}
