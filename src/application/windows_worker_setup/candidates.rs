//! Guest address hints from the receipt, filtered to the recorded management
//! network. Candidate probing is bounded and never scans the network: only the
//! addresses the guest itself reported are considered.
use super::receipt::{ConnectionReceipt, MAX_CANDIDATE_ADDRESSES};
use crate::application::worker_media::GenerationRecord;
use std::net::Ipv4Addr;

/// Project the receipt's address hints into the connection attempts worth making.
///
/// An address is kept only when it is a unicast IPv4 address inside the recorded
/// management network (`record.management_peer` / `record.network_prefix_len`).
/// The result is de-duplicated, numerically ordered and capped. A previously
/// working address is tried first so a refreshed receipt does not lose the known
/// path when the guest picks up additional addresses.
pub fn candidates_for(
    record: &GenerationRecord,
    receipt: &ConnectionReceipt,
    preferred: Option<&str>,
) -> Vec<Ipv4Addr> {
    let network = network_address(record);
    let mut addresses: Vec<Ipv4Addr> = receipt
        .candidate_addresses
        .iter()
        .filter_map(|candidate| candidate.trim().parse::<Ipv4Addr>().ok())
        .filter(|address| {
            eligible(*address) && in_network(*address, network, record.network_prefix_len)
        })
        .collect();
    addresses.sort_unstable();
    addresses.dedup();
    if let Some(preferred) = preferred
        .and_then(|preferred| preferred.trim().parse::<Ipv4Addr>().ok())
        .filter(|preferred| addresses.contains(preferred))
    {
        addresses.retain(|address| *address != preferred);
        addresses.insert(0, preferred);
    }
    addresses.truncate(MAX_CANDIDATE_ADDRESSES);
    addresses
}

fn network_address(record: &GenerationRecord) -> u32 {
    u32::from(record.management_peer) & mask(record.network_prefix_len)
}

fn mask(prefix_len: u8) -> u32 {
    if prefix_len >= 32 {
        u32::MAX
    } else {
        u32::MAX << (32 - u32::from(prefix_len))
    }
}

fn in_network(address: Ipv4Addr, network: u32, prefix_len: u8) -> bool {
    (u32::from(address) & mask(prefix_len)) == network
}

fn eligible(address: Ipv4Addr) -> bool {
    !address.is_loopback()
        && !address.is_unspecified()
        && !address.is_multicast()
        && !address.is_broadcast()
        && !address.is_link_local()
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use std::path::PathBuf;
    use wukong_worker_package::SshPublicKey;

    fn record(management_peer: [u8; 4], network_prefix_len: u8) -> GenerationRecord {
        GenerationRecord {
            schema: 1,
            installation_id: "0123456789abcdef0123456789abcdef".into(),
            environment_id: "windows-01234567".into(),
            app_version: "1.0.0".into(),
            template_sha256: "a".repeat(64),
            payload_manifest_sha256: "b".repeat(64),
            controller_public_key: "c".repeat(64),
            ssh_identity_file: PathBuf::from("C:\\state\\ssh\\management_ed25519"),
            management_peer: Ipv4Addr::from(management_peer),
            network_prefix_len,
            ssh_port: 22_240,
            media_path: PathBuf::from("C:\\state\\media"),
            zip_path: PathBuf::from("C:\\state\\media.zip"),
            created_at: "2026-01-01T00:00:00+00:00".into(),
        }
    }

    fn receipt(candidates: &[&str]) -> ConnectionReceipt {
        let mut blob = vec![0, 0, 0, 11];
        blob.extend_from_slice(b"ssh-ed25519");
        blob.extend_from_slice(&[0, 0, 0, 32]);
        blob.extend_from_slice(&[7; 32]);
        let key = SshPublicKey {
            algorithm: "ssh-ed25519".into(),
            key: base64::engine::general_purpose::STANDARD.encode(blob),
        };
        let fingerprint = key.fingerprint().expect("fixture fingerprint");
        ConnectionReceipt {
            schema: 1,
            receipt_version: 2,
            installed: true,
            installation_id: "0123456789abcdef0123456789abcdef".into(),
            environment_id: "windows-01234567".into(),
            generation: 1,
            manifest_sha256: "b".repeat(64),
            ssh_user: format!("wkgm{}", "0123456789abcdef"),
            ssh_port: 22_240,
            host_key: key,
            host_fingerprint: fingerprint,
            candidate_addresses: candidates.iter().map(|value| (*value).into()).collect(),
            production_isolation_verified: false,
        }
    }

    fn values(addresses: &[Ipv4Addr]) -> Vec<String> {
        addresses
            .iter()
            .map(std::string::ToString::to_string)
            .collect()
    }

    #[test]
    fn candidates_outside_the_management_network_or_non_unicast_are_dropped() {
        let record = record([192, 168, 199, 1], 24);
        let receipt = receipt(&[
            "192.168.199.40",
            "192.168.200.7",
            "10.0.0.5",
            "127.0.0.1",
            "169.254.3.4",
            "224.0.0.1",
            "not-an-address",
        ]);
        assert_eq!(
            values(&candidates_for(&record, &receipt, None)),
            vec!["192.168.199.40"]
        );
    }

    #[test]
    fn candidates_are_deduplicated_and_numerically_ordered() {
        let record = record([192, 168, 199, 1], 24);
        let receipt = receipt(&[
            "192.168.199.150",
            "192.168.199.20",
            "192.168.199.20",
            "192.168.199.3",
        ]);
        assert_eq!(
            values(&candidates_for(&record, &receipt, None)),
            vec!["192.168.199.3", "192.168.199.20", "192.168.199.150"]
        );
    }

    #[test]
    fn a_previously_working_address_is_tried_first() {
        let record = record([192, 168, 199, 1], 24);
        let receipt = receipt(&["192.168.199.150", "192.168.199.20", "192.168.199.3"]);
        assert_eq!(
            values(&candidates_for(&record, &receipt, Some("192.168.199.20"))),
            vec!["192.168.199.20", "192.168.199.3", "192.168.199.150"]
        );
        assert_eq!(
            values(&candidates_for(&record, &receipt, Some("192.168.200.3"))),
            vec!["192.168.199.3", "192.168.199.20", "192.168.199.150"]
        );
    }

    #[test]
    fn a_wider_recorded_prefix_keeps_its_addresses_and_the_cap_holds() {
        let record = record([172, 20, 0, 1], 20);
        let receipt = receipt(&[
            "172.20.15.7",
            "172.21.0.1",
            "172.20.1.1",
            "172.20.2.1",
            "172.20.3.1",
            "172.20.4.1",
            "172.20.5.1",
            "172.20.6.1",
            "172.20.7.1",
            "172.20.8.1",
        ]);
        let candidates = candidates_for(&record, &receipt, None);
        assert_eq!(
            values(&candidates),
            vec![
                "172.20.1.1",
                "172.20.2.1",
                "172.20.3.1",
                "172.20.4.1",
                "172.20.5.1",
                "172.20.6.1",
                "172.20.7.1",
                "172.20.8.1"
            ]
        );
    }

    #[test]
    fn a_receipt_without_hints_yields_no_attempts() {
        let record = record([192, 168, 199, 1], 24);
        assert!(candidates_for(&record, &receipt(&[]), None).is_empty());
        assert!(
            candidates_for(&record, &receipt(&["203.0.113.9"]), None).is_empty(),
            "a public address outside the management network is never scanned"
        );
    }
}
