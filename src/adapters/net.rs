//! Host IPv4 interface enumeration for choosing the worker management network.
//!
//! The desktop controller reaches an installed Windows worker through one of
//! this host's IPv4 interfaces, usually a VM host-only adapter. `GetAdaptersAddresses`
//! reports every adapter with its unicast addresses and on-link prefix lengths;
//! this module projects that table into deterministic, operator-selectable
//! management networks. Only enumeration touches the operating system: address
//! filtering, ordering and the recommendation are pure functions so tests never
//! depend on the machine's live adapters.
use anyhow::Result;
use serde::Serialize;
use std::net::Ipv4Addr;

/// One IPv4 unicast address of a host adapter and its on-link prefix length.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostAddress {
    pub ipv4: Ipv4Addr,
    pub prefix_len: u8,
}

/// One host adapter as reported by the platform, already reduced to IPv4.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostAdapter {
    pub id: String,
    pub name: String,
    pub description: String,
    /// Down adapters keep stale addresses; the projection only uses live ones.
    pub up: bool,
    pub addresses: Vec<HostAddress>,
}

/// One selectable management network: an adapter's IPv4 address and prefix.
///
/// The address is what the guest uses to reach this controller; it is never a
/// guest address. Public addresses are listed but only RFC 1918 addresses are
/// recommended for a VM management network.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ManagementNetwork {
    /// Stable adapter identity from the platform adapter GUID.
    pub id: String,
    pub name: String,
    pub description: String,
    pub ipv4: Ipv4Addr,
    pub prefix_len: u8,
    pub rfc1918: bool,
    pub virtual_adapter: bool,
    /// A default choice for the GUI; at most one projected network is recommended.
    pub recommended: bool,
}

/// Injectable adapter table source; tests provide fixed tables.
pub trait AdapterSource {
    fn adapters(&self) -> Result<Vec<HostAdapter>>;
}

/// Live host adapter table.
pub struct SystemAdapters;

impl AdapterSource for SystemAdapters {
    fn adapters(&self) -> Result<Vec<HostAdapter>> {
        #[cfg(windows)]
        {
            windows::adapters()
        }
        #[cfg(not(windows))]
        {
            anyhow::bail!("host IPv4 enumeration is only implemented for Windows")
        }
    }
}

/// Enumerate this host's eligible management networks.
pub fn management_networks(source: &dyn AdapterSource) -> Result<Vec<ManagementNetwork>> {
    Ok(project(&source.adapters()?))
}

/// Project an adapter table into management networks.
///
/// Eligible addresses are every live, non-loopback, non-link-local (APIPA),
/// unicast IPv4 address. The result is ordered by recommendation rank and then
/// by numeric address so the GUI and the automatic single-network selection are
/// deterministic. Exactly one RFC 1918 network is marked `recommended`: the best
/// virtual-machine host-only adapter when there is one, then another hypervisor
/// adapter (Hyper-V's shared Default Switch ranks below dedicated VM networks),
/// otherwise the lowest private address.
pub(crate) fn project(adapters: &[HostAdapter]) -> Vec<ManagementNetwork> {
    let mut networks = Vec::new();
    for adapter in adapters.iter().filter(|adapter| adapter.up) {
        let tier = adapter_tier(&adapter.name, &adapter.description);
        let virtual_adapter = tier < PHYSICAL_TIER;
        let name = if adapter.name.trim().is_empty() {
            if adapter.description.trim().is_empty() {
                adapter.id.clone()
            } else {
                adapter.description.trim().to_owned()
            }
        } else {
            adapter.name.trim().to_owned()
        };
        for address in &adapter.addresses {
            if !eligible(address.ipv4) {
                continue;
            }
            let network = ManagementNetwork {
                id: adapter.id.clone(),
                name: name.clone(),
                description: adapter.description.trim().to_owned(),
                ipv4: address.ipv4,
                prefix_len: address.prefix_len,
                rfc1918: address.ipv4.is_private(),
                virtual_adapter,
                recommended: false,
            };
            if !networks
                .iter()
                .any(|(existing, _): &(ManagementNetwork, u8)| {
                    existing.id == network.id && existing.ipv4 == network.ipv4
                })
            {
                networks.push((network, tier));
            }
        }
    }
    networks.sort_by_key(|(network, tier)| {
        (
            recommendation_rank(network.rfc1918, network.virtual_adapter),
            *tier,
            u32::from(network.ipv4),
        )
    });
    let mut networks: Vec<ManagementNetwork> =
        networks.into_iter().map(|(network, _)| network).collect();
    // The sort already puts every RFC 1918 entry before public ones, so the first
    // private entry is the best-ranked management network.
    if let Some(index) = networks.iter().position(|network| network.rfc1918) {
        networks[index].recommended = true;
    }
    networks
}

fn eligible(address: Ipv4Addr) -> bool {
    !address.is_loopback()
        && !address.is_unspecified()
        && !address.is_multicast()
        && !address.is_broadcast()
        && !address.is_link_local()
}

fn recommendation_rank(rfc1918: bool, virtual_adapter: bool) -> u8 {
    match (rfc1918, virtual_adapter) {
        (true, true) => 0,
        (true, false) => 1,
        (false, true) => 2,
        (false, false) => 3,
    }
}

/// Tier of an adapter as a VM management-network candidate; lower is better.
const PHYSICAL_TIER: u8 = 3;

/// Adapter names/descriptions are classified so dedicated VM host-only networks
/// outrank Hyper-V's shared Default Switch, which is frequently a NAT network a
/// manually installed VM cannot be reached through.
fn adapter_tier(name: &str, description: &str) -> u8 {
    let text = format!("{name} {description}").to_ascii_lowercase();
    if ["vmnet", "host-only", "hostonly"]
        .iter()
        .any(|hint| text.contains(hint))
    {
        0
    } else if ["vmware", "virtualbox"]
        .iter()
        .any(|hint| text.contains(hint))
    {
        1
    } else if ["hyper-v", "vethernet"]
        .iter()
        .any(|hint| text.contains(hint))
    {
        2
    } else {
        PHYSICAL_TIER
    }
}

#[cfg(windows)]
mod windows {
    use super::{HostAdapter, HostAddress};
    use anyhow::{Result, ensure};
    use std::{mem::size_of, net::Ipv4Addr};
    use windows_sys::Win32::{
        Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_NO_DATA},
        NetworkManagement::{
            IpHelper::{
                GAA_FLAG_INCLUDE_PREFIX, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER,
                GAA_FLAG_SKIP_MULTICAST, GetAdaptersAddresses, IP_ADAPTER_ADDRESSES_LH,
                IP_ADAPTER_UNICAST_ADDRESS_LH,
            },
            Ndis::IfOperStatusUp,
        },
        Networking::WinSock::{AF_INET, AF_UNSPEC, SOCKADDR_IN},
    };

    /// Upper bound for the adapter table the SDK may fill.
    const MAXIMUM_TABLE_BYTES: u32 = 4 * 1024 * 1024;
    /// Upper bound for adapter/address record traversal.
    const MAXIMUM_ADAPTERS: usize = 1024;
    const MAXIMUM_UNICAST: usize = 64;
    /// Adapter strings are short; the cap only bounds a malformed SDK table.
    const MAXIMUM_STRING_UNITS: usize = 512;

    pub(super) fn adapters() -> Result<Vec<HostAdapter>> {
        let flags = GAA_FLAG_INCLUDE_PREFIX
            | GAA_FLAG_SKIP_ANYCAST
            | GAA_FLAG_SKIP_DNS_SERVER
            | GAA_FLAG_SKIP_MULTICAST;
        let family = u32::from(AF_UNSPEC);
        let mut size = 0u32;
        // SAFETY: a null table with a valid out-parameter is the documented size
        // probe; the call writes only the bounded required-size value into `size`.
        let status = unsafe {
            GetAdaptersAddresses(
                family,
                flags,
                std::ptr::null(),
                std::ptr::null_mut(),
                &mut size,
            )
        };
        if status == 0 || status == ERROR_NO_DATA {
            return Ok(Vec::new());
        }
        ensure!(
            status == ERROR_BUFFER_OVERFLOW,
            "cannot size the host adapter table (Windows error {status})"
        );
        ensure!(
            size > 0 && size <= MAXIMUM_TABLE_BYTES,
            "host adapter table exceeds its inspection limit"
        );
        // `u64` slots keep the buffer aligned for the pointer-bearing records.
        let mut table = vec![
            0u64;
            usize::try_from(size)
                .unwrap_or(usize::MAX)
                .div_ceil(size_of::<u64>())
        ];
        // SAFETY: `table` is pointer-aligned and spans at least `size` bytes; the
        // SDK writes only `size` bytes of records into it and updates `size`.
        let status = unsafe {
            GetAdaptersAddresses(
                family,
                flags,
                std::ptr::null(),
                table.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>(),
                &mut size,
            )
        };
        ensure!(
            status == 0,
            "cannot read the host adapter table (Windows error {status})"
        );
        let mut output = Vec::new();
        // SAFETY: a successful call fills `size` bytes of adapter records starting
        // at the buffer base, and every `Next` link stays inside that table.
        let mut adapter = table.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        while !adapter.is_null() {
            ensure!(
                output.len() < MAXIMUM_ADAPTERS,
                "host adapter table exceeds its entry limit"
            );
            // SAFETY: `adapter` is a non-null record inside the SDK-filled table.
            let raw = unsafe { &*adapter };
            let up = raw.OperStatus == IfOperStatusUp;
            // SAFETY: `AdapterName` points at a NUL-terminated ANSI GUID string in
            // the SDK-filled table; the reader stops at the NUL or its cap.
            let id = unsafe { ansi_string(raw.AdapterName) };
            // SAFETY: `FriendlyName` points at a NUL-terminated wide string in the
            // same table; the reader stops at the NUL or its cap.
            let name = unsafe { wide_string(raw.FriendlyName) };
            // SAFETY: `Description` is a NUL-terminated wide string in the same table.
            let description = unsafe { wide_string(raw.Description) };
            let mut addresses = Vec::new();
            let mut unicast = raw.FirstUnicastAddress;
            while !unicast.is_null() {
                ensure!(
                    addresses.len() < MAXIMUM_UNICAST,
                    "host adapter address list exceeds its entry limit"
                );
                // SAFETY: `unicast` is a non-null node inside the SDK-filled table.
                let entry = unsafe { &*unicast };
                if let Some(address) = ipv4(entry) {
                    addresses.push(address);
                }
                unicast = entry.Next;
            }
            output.push(HostAdapter {
                id,
                name,
                description,
                up,
                addresses,
            });
            adapter = raw.Next;
        }
        Ok(output)
    }

    /// Read one IPv4 unicast entry; other address families are ignored.
    fn ipv4(entry: &IP_ADAPTER_UNICAST_ADDRESS_LH) -> Option<HostAddress> {
        let socket = entry.Address;
        // SAFETY: `lpSockaddr` and its family tag are inline in the SDK table.
        if socket.lpSockaddr.is_null() {
            return None;
        }
        let length = usize::try_from(socket.iSockaddrLength).ok()?;
        if length < size_of::<SOCKADDR_IN>() {
            return None;
        }
        // SAFETY: the length check above proves at least a SOCKADDR_IN-sized
        // address; the family tag is the first field of SOCKADDR and SOCKADDR_IN.
        let sockaddr = unsafe { &*socket.lpSockaddr.cast::<SOCKADDR_IN>() };
        if sockaddr.sin_family != AF_INET {
            return None;
        }
        // SAFETY: the family tag matched, so reading the IPv4 union member of the
        // same SOCKADDR_IN record is valid.
        let octets = u32::from_be(unsafe { sockaddr.sin_addr.S_un.S_addr });
        Some(HostAddress {
            ipv4: Ipv4Addr::from(octets),
            prefix_len: entry.OnLinkPrefixLength.min(32),
        })
    }

    /// Read a bounded NUL-terminated ANSI string from an SDK-owned table.
    unsafe fn ansi_string(pointer: windows_sys::core::PSTR) -> String {
        if pointer.is_null() {
            return String::new();
        }
        let mut bytes = Vec::new();
        for index in 0..MAXIMUM_STRING_UNITS {
            // SAFETY: the platform guarantees the pointer is a NUL-terminated
            // string inside the live table; the loop stops at the NUL or the cap.
            let byte = unsafe { *pointer.add(index) };
            if byte == 0 {
                break;
            }
            bytes.push(byte);
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// Read a bounded NUL-terminated UTF-16 string from an SDK-owned table.
    unsafe fn wide_string(pointer: windows_sys::core::PWSTR) -> String {
        if pointer.is_null() {
            return String::new();
        }
        let mut units = Vec::new();
        for index in 0..MAXIMUM_STRING_UNITS {
            // SAFETY: the platform guarantees the pointer is a NUL-terminated
            // string inside the live table; the loop stops at the NUL or the cap.
            let unit = unsafe { *pointer.add(index) };
            if unit == 0 {
                break;
            }
            units.push(unit);
        }
        String::from_utf16_lossy(&units)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(ipv4: &str, prefix_len: u8) -> HostAddress {
        HostAddress {
            ipv4: ipv4.parse().expect("fixture address"),
            prefix_len,
        }
    }

    fn adapter(
        id: &str,
        name: &str,
        description: &str,
        addresses: Vec<HostAddress>,
    ) -> HostAdapter {
        HostAdapter {
            id: id.into(),
            name: name.into(),
            description: description.into(),
            up: true,
            addresses,
        }
    }

    #[test]
    fn projection_filters_link_local_loopback_and_down_adapters() {
        let mut down = adapter(
            "down",
            "Ethernet 2",
            "Disconnected adapter",
            vec![address("10.9.9.9", 24)],
        );
        down.up = false;
        let networks = project(&[
            adapter(
                "loopback",
                "Loopback Pseudo-Interface 1",
                "Software loopback",
                vec![address("127.0.0.1", 8)],
            ),
            down,
            adapter(
                "apipa",
                "Ethernet",
                "Media disconnected",
                vec![address("169.254.20.30", 16)],
            ),
            adapter(
                "wifi",
                "Wi-Fi",
                "Intel Wireless",
                vec![
                    address("192.168.1.20", 24),
                    address("0.0.0.0", 0),
                    address("255.255.255.255", 0),
                    address("224.0.0.1", 0),
                ],
            ),
        ]);
        assert_eq!(networks.len(), 1);
        assert_eq!(networks[0].id, "wifi");
        assert_eq!(networks[0].ipv4, Ipv4Addr::new(192, 168, 1, 20));
        assert_eq!(networks[0].prefix_len, 24);
        assert!(networks[0].rfc1918);
        assert!(networks[0].recommended);
    }

    #[test]
    fn the_virtual_host_only_network_is_recommended_and_ordered_first() {
        let networks = project(&[
            adapter(
                "wifi",
                "Wi-Fi",
                "Intel Wireless",
                vec![address("192.168.1.20", 24)],
            ),
            adapter(
                "vmnet",
                "VMware Network Adapter VMnet8",
                "VMware Virtual Ethernet Adapter for VMnet8",
                vec![address("192.168.199.1", 24)],
            ),
            adapter(
                "public",
                "Ethernet",
                "USB Ethernet",
                vec![address("203.0.113.7", 24)],
            ),
            adapter(
                "hyperv",
                "vEthernet (Default Switch)",
                "Hyper-V Virtual Ethernet Adapter",
                vec![address("172.20.0.1", 20)],
            ),
        ]);
        assert_eq!(
            networks
                .iter()
                .map(|network| network.id.as_str())
                .collect::<Vec<_>>(),
            vec!["vmnet", "hyperv", "wifi", "public"]
        );
        assert!(networks[0].recommended);
        assert!(networks[1].virtual_adapter);
        assert!(!networks[2].virtual_adapter);
        assert!(!networks[3].rfc1918);
        assert_eq!(
            networks
                .iter()
                .filter(|network| network.recommended)
                .count(),
            1
        );
    }

    #[test]
    fn a_single_private_network_is_recommended_and_public_only_lists_nothing() {
        let networks = project(&[adapter(
            "ethernet",
            "Ethernet",
            "Realtek",
            vec![address("10.1.2.3", 16)],
        )]);
        assert!(networks[0].recommended);

        let public = project(&[adapter(
            "ethernet",
            "Ethernet",
            "Realtek",
            vec![address("203.0.113.7", 24)],
        )]);
        assert_eq!(public.len(), 1);
        assert!(!public[0].recommended);
        assert!(!public[0].rfc1918);
    }

    #[test]
    fn multiple_addresses_on_one_adapter_are_sorted_and_deduplicated() {
        let networks = project(&[
            adapter(
                "ethernet",
                "Ethernet",
                "Realtek",
                vec![address("192.168.5.9", 24)],
            ),
            adapter(
                "ethernet",
                "Ethernet",
                "Realtek",
                vec![address("192.168.5.9", 24), address("192.168.4.2", 24)],
            ),
            adapter("ethernet", "", "Realtek", vec![address("192.168.4.2", 24)]),
        ]);
        assert_eq!(
            networks
                .iter()
                .map(|network| network.ipv4.to_string())
                .collect::<Vec<_>>(),
            vec!["192.168.4.2", "192.168.5.9"]
        );
        assert_eq!(networks[0].name, "Ethernet");
    }
}
