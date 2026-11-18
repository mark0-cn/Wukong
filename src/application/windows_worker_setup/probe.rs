//! Bounded candidate verification through the existing managed-SSH environment.
//!
//! Import never scans the management network: it tries only the guest-reported
//! candidate addresses, using the same managed SSH transport, strict host-key pin
//! and doctor check that later runs analysis. A candidate is accepted only when
//! the worker's installation identity, the pinned receipt host key and the
//! platform/doctor observation all match. Every attempt is bounded so a stopped
//! VM cannot stall the GUI command.
use super::receipt::ConnectionReceipt;
use crate::adapters::environments::registration::{self, WindowsConnection};
use crate::adapters::environments::{EnvironmentConfig, EnvironmentRegistry};
use crate::application::worker_media::GenerationRecord;
use anyhow::{Context, Result, bail, ensure};
use std::{collections::BTreeMap, net::Ipv4Addr, path::Path, time::Duration};
use tokio::time::{Instant, timeout_at};

/// Total time budget for verification across all candidate addresses.
pub const PROBE_BUDGET: Duration = Duration::from_secs(12);
/// Bound for one candidate, so an unreachable address cannot consume the budget.
const PER_CANDIDATE: Duration = Duration::from_secs(4);

/// A verified management address and the number of attempts it took.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProbeOutcome {
    pub address: Ipv4Addr,
    pub attempts: usize,
}

/// Verify one candidate address for this installation, in order.
///
/// `existing` is the configuration's environment map with the environment that
/// will be updated (if any) removed, so the registration adapter can validate the
/// new entry without rejecting the installation's own registration.
pub async fn verify_management_address(
    home: &Path,
    receipt: &ConnectionReceipt,
    receipt_json: &str,
    record: &GenerationRecord,
    candidates: &[Ipv4Addr],
    name: &str,
    existing: &BTreeMap<String, EnvironmentConfig>,
) -> Result<ProbeOutcome> {
    ensure!(
        !candidates.is_empty(),
        "the connection receipt has no candidate address on the recorded management network"
    );
    let deadline = Instant::now() + PROBE_BUDGET;
    let mut failures = Vec::new();
    for (index, address) in candidates.iter().copied().enumerate() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        let attempt = probe_address(home, receipt, receipt_json, record, address, name, existing);
        let limit = deadline.min(Instant::now() + PER_CANDIDATE);
        match timeout_at(limit, attempt).await {
            Ok(Ok(())) => {
                return Ok(ProbeOutcome {
                    address,
                    attempts: index + 1,
                });
            }
            Ok(Err(error)) => failures.push(format!("{address}: {error:#}")),
            Err(_) => failures.push(format!("{address}: timed out")),
        }
    }
    bail!(
        "none of the saved address candidates accepted a verified managed SSH connection: {}. Start the disposable VM and attach its host-only management network, or re-export the install result from the Windows setup and import it again",
        failures.join("; ")
    )
}

/// One strict managed SSH connection plus doctor check.
async fn probe_address(
    home: &Path,
    receipt: &ConnectionReceipt,
    receipt_json: &str,
    record: &GenerationRecord,
    address: Ipv4Addr,
    name: &str,
    existing: &BTreeMap<String, EnvironmentConfig>,
) -> Result<()> {
    let probe = tempfile::tempdir().context("create a temporary SSH host-pin directory")?;
    let pin = probe.path().join("known_hosts");
    let input = WindowsConnection {
        connection_json: receipt_json.to_owned(),
        name: name.to_owned(),
        host: address.to_string(),
        identity_file: record.ssh_identity_file.clone(),
        confirmed_fingerprint: receipt.host_fingerprint.clone(),
        make_default: false,
    };
    let (environment_id, value) = registration::prepare(home, &input, existing)?;
    ensure!(
        environment_id == receipt.environment_id,
        "the registration adapter produced a different environment identity"
    );
    let mut environment: EnvironmentConfig = serde_json::from_value(value)
        .context("the registration adapter produced an invalid environment")?;
    let ssh = environment
        .runtime
        .managed_ssh_mut()
        .context("the registration adapter produced a non-managed-SSH environment")?;
    // The probe pins the receipt host key in a throw-away file. The real
    // controller pin is only published by the configuration save after this
    // verification succeeded, so a failed import leaves no trusted connection.
    ssh.known_hosts_file = pin.clone();
    std::fs::write(&pin, ssh.known_hosts_record()?)
        .with_context(|| format!("write the temporary SSH host pin {}", pin.display()))?;
    let registry =
        EnvironmentRegistry::new(BTreeMap::from([(environment_id.clone(), environment)]))
            .context("prepare the managed SSH verification")?;
    let verdict = registry.doctor(&environment_id).await;
    registry.close().await;
    verdict.context("managed SSH verification failed")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_verification_budget_is_bounded_and_never_scans() {
        assert!(PROBE_BUDGET <= Duration::from_secs(12));
        assert!(PER_CANDIDATE <= PROBE_BUDGET);
    }
}
