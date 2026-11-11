//! Signed offline worker package format, shared by the controller and native installer.
mod files;
pub use files::verify_package;
mod manifest;
mod paths;
mod signatures;

pub use manifest::{Compatibility, Component, EntryPoints, FileEntry, Manifest, PayloadKind};
pub use paths::validate_path;
pub use signatures::{PublicKey, SignatureRecord, Signer, VerifiedManifest};
