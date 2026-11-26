//! Signed offline worker package format, shared by the controller and native installer.
mod connection;
pub use connection::{
    ConnectionReceipt, MAX_CANDIDATE_ADDRESSES, MAX_RECEIPT_BYTES, RECEIPT_VERSION,
};
mod enrollment;
mod files;
pub use enrollment::{
    Enrollment, ExecutionBoundary, SshPublicKey, VerifiedEnrollment, write_enrollment,
};
pub use files::{verify_package, verify_package_for_upgrade};
mod keys;
pub use keys::{create_controller_signer, load_controller_signer};
mod seal;
pub use seal::seal_package;
mod manifest;
mod paths;
mod signatures;

pub use manifest::{Compatibility, Component, EntryPoints, FileEntry, Manifest, PayloadKind};
pub use paths::validate_path;
pub use signatures::{PublicKey, SignatureRecord, Signer, VerifiedManifest};
