use crate::{Compatibility, Manifest, Signer, VerifiedManifest, files, verify_package};
use anyhow::Result;
use std::{fs, io::Write, path::Path};

/// Operator-only sealing of a private, complete payload directory. No worker
/// RPC exposes signing or trust enrollment. Partial metadata is retained on IO
/// failure for diagnosis; this entry never overwrites/reseals an existing tree.
pub fn seal_package(
    root: &Path,
    manifest: &Manifest,
    signer: &Signer,
    expected: &Compatibility<'_>,
) -> Result<VerifiedManifest> {
    manifest.check_compatibility(expected)?;
    files::verify_payload_tree(root, manifest, None)?;
    let (bytes, signature) = signer.sign(manifest)?;
    let signature = serde_json::to_vec(&signature)?;
    let public = format!("{}\n", signer.public_key().to_hex());
    for (name, bytes) in [
        ("manifest.json", bytes.as_slice()),
        ("manifest.sig", signature.as_slice()),
        ("manifest.pub", public.as_bytes()),
    ] {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(root.join(name))?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    verify_package(root, &signer.public_key(), expected)
}
