//! Bind actual workers transactionally on first use and preserve restart history.
use super::*;

impl Store {
    pub(crate) fn bind_environment(&self, binding: &EnvironmentBinding) -> Result<()> {
        self.write(|session| {
            if let Some(previous) = session.get_record::<EnvironmentBinding>(&binding.id)? {
                anyhow::ensure!(
                    previous.run_id == binding.run_id
                        && previous.environment_id == binding.environment_id,
                    "environment assignment changed since first use; start a new assessment"
                );
                if previous.identity != binding.identity {
                    anyhow::ensure!(
                        previous.identity.same_installation(&binding.identity),
                        "environment installation, package or protocol identity changed since first use; start a new assessment"
                    );
                    let mut updated = previous;
                    if !updated.prior_identities.contains(&updated.identity)
                        && updated.prior_identities.len() < 16
                    {
                        updated.prior_identities.push(updated.identity.clone());
                    }
                    updated.identity = binding.identity.clone();
                    session.save_record(&updated)?;
                }
            } else {
                session.save_record(binding)?;
            }
            Ok(())
        })
    }

    pub(crate) fn controller_owner(&self) -> Option<String> {
        self.fence
            .as_ref()
            .map(|token| format!("{}:{}", token.owner, token.epoch()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resumed_environment_allows_boot_restart_but_not_installation_or_content_change() -> Result<()>
    {
        let root = tempfile::tempdir()?;
        let store = Store::open(root.path())?;
        let binding = EnvironmentBinding {
            id: "binding".into(),
            run_id: "run".into(),
            environment_id: "windows".into(),
            identity: crate::adapters::worker::protocol::WorkerIdentity {
                protocol: 4,
                installation_id: "installation".into(),
                boot_id: "boot".into(),
                package_sha256: "0".repeat(64),
            },
            prior_identities: vec![],
        };
        store.bind_environment(&binding)?;
        let resumed = Store::open(root.path())?;
        resumed.bind_environment(&binding)?;
        let mut restarted = binding.clone();
        restarted.identity.boot_id = "restarted".into();
        resumed.bind_environment(&restarted)?;
        let current = resumed.load::<EnvironmentBinding>(&binding.id)?;
        assert_eq!(current.identity, restarted.identity);
        assert_eq!(current.prior_identities, vec![binding.identity.clone()]);
        for field in ["installation", "package"] {
            let mut changed = restarted.clone();
            match field {
                "installation" => changed.identity.installation_id = "another".into(),
                _ => changed.identity.package_sha256 = "1".repeat(64),
            }
            assert!(resumed.bind_environment(&changed).is_err());
        }
        assert_eq!(
            resumed.load::<EnvironmentBinding>(&binding.id)?.identity,
            restarted.identity
        );
        Ok(())
    }
}
