//! Bind actual workers transactionally on first use; later resumes cannot rebind.
use super::*;

impl Store {
    pub(crate) fn bind_environment(&self, binding: &EnvironmentBinding) -> Result<()> {
        self.write(|session| {
            if let Some(previous) = session.get_record::<EnvironmentBinding>(&binding.id)? {
                anyhow::ensure!(previous.run_id == binding.run_id
                    && previous.environment_id == binding.environment_id
                    && previous.identity == binding.identity,
                    "environment installation, boot or package identity changed since first use; start a new assessment");
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
    fn resumed_environment_cannot_change_installation_boot_or_content() -> Result<()> {
        let root = tempfile::tempdir()?;
        let store = Store::open(root.path())?;
        let binding = EnvironmentBinding {
            id: "binding".into(),
            run_id: "run".into(),
            environment_id: "windows".into(),
            identity: crate::runtime::protocol::WorkerIdentity {
                protocol: 2,
                installation_id: "installation".into(),
                boot_id: "boot".into(),
                package_sha256: "0".repeat(64),
            },
        };
        store.bind_environment(&binding)?;
        let resumed = Store::open(root.path())?;
        resumed.bind_environment(&binding)?;
        for field in ["installation", "boot", "package"] {
            let mut changed = binding.clone();
            match field {
                "installation" => changed.identity.installation_id = "another".into(),
                "boot" => changed.identity.boot_id = "another".into(),
                _ => changed.identity.package_sha256 = "1".repeat(64),
            }
            assert!(resumed.bind_environment(&changed).is_err());
        }
        assert_eq!(
            resumed.load::<EnvironmentBinding>(&binding.id)?.identity,
            binding.identity
        );
        Ok(())
    }
}
