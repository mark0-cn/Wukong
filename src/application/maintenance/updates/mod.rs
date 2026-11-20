//! Signed release identities and recoverable desktop maintenance.
pub mod environment;
#[cfg(feature = "desktop")]
pub(crate) mod health;
#[cfg(feature = "local-release")]
pub mod local_release;
pub mod maintenance;
pub mod manifest;
pub mod recovery;
pub mod storage;

/// Environment identity a first-run setup binds to.
///
/// Online builds resolve it from the signed release catalog; `local-release`
/// builds resolve it from their bundled descriptor and a local Docker image.
pub struct OnboardingRelease {
    pub version: String,
    pub image: String,
    /// Image size observed on this Docker host (local builds) or published by
    /// the signed catalog (online builds).
    pub image_size: u64,
    /// True when the identity came from the bundled local-release descriptor.
    pub local_build: bool,
}
