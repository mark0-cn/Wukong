use crate::{
    host::deployment::Deployment,
    isolation::security::{self, AclPolicy},
};
use anyhow::Result;
use std::path::{Path, PathBuf};
/// Paths and installation identity injected only after host deployment verification.
#[derive(Clone)]
pub(crate) struct RuntimeContext {
    pub(crate) artifacts: crate::artifacts::outputs::Artifacts,
    pub(crate) root: PathBuf,
    pub(crate) executable: PathBuf,
    pub(crate) installation: String,
}
impl RuntimeContext {
    pub(crate) fn verified(
        root: &Path,
        deployment: &Deployment,
        manifest: &wukong_worker_package::VerifiedManifest,
    ) -> Result<Self> {
        security::require_system()?;
        let root = root.canonicalize()?;
        let private = AclPolicy::private_vm();
        for name in ["resource-requests", "resource-results"] {
            let directory = root.join("metadata").join(name);
            if !directory.exists() {
                private.create_directory(&directory)?;
            }
            private.verify(&directory, true)?;
        }
        Ok(Self {
            executable: deployment
                .package(&root)
                .join(&manifest.manifest().entrypoints.worker),
            artifacts: crate::artifacts::outputs::Artifacts { root: root.clone() },
            root,
            installation: deployment.installation_id.clone(),
        })
    }
}
