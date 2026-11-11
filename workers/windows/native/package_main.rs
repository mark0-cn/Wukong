//! Controller operator utility. This binary is never part of VM runtime payloads.
use anyhow::{Context, Result, bail, ensure};
use serde_json::json;
use std::{fs, io::Read, path::Path};
use wukong_worker_package::{Compatibility, Manifest, load_controller_signer, seal_package};

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().collect();
    match (args.get(1).and_then(|value| value.to_str()), args.len()) {
        (Some("key"), 3) => {
            let signer = load_controller_signer(Path::new(&args[2]))?;
            println!(
                "{}",
                json!({"public_key":signer.public_key().to_hex(),"key_sha256":signer.public_key().fingerprint()})
            );
        }
        (Some("seal"), 6) => {
            let root_path = Path::new(&args[2]);
            let root = root_path.canonicalize()?;
            let draft = Path::new(&args[3]).canonicalize()?;
            let keys = Path::new(&args[4]);
            ensure!(
                keys.is_absolute(),
                "controller key directory must be absolute"
            );
            // canonicalize() adds a Windows long-path prefix. Normalize both
            // sides before checking containment; comparing a raw C:\ path to
            // a \\?\ path can otherwise miss a key directory inside the payload.
            let key_destination = if keys.try_exists()? {
                keys.canonicalize()?
            } else {
                keys.parent()
                    .context("missing controller key parent")?
                    .canonicalize()?
                    .join(
                        keys.file_name()
                            .context("missing controller key directory name")?,
                    )
            };
            ensure!(
                !key_destination.starts_with(&root) && !draft.starts_with(&root),
                "controller keys and draft must stay outside package"
            );
            let mut bytes = Vec::new();
            fs::File::open(draft)?
                .take(4 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)?;
            ensure!(
                bytes.len() <= 4 * 1024 * 1024,
                "manifest draft exceeds size limit"
            );
            let manifest: Manifest = serde_json::from_slice(&bytes)?;
            let signer = load_controller_signer(keys)?;
            let result = seal_package(
                root_path,
                &manifest,
                &signer,
                &Compatibility {
                    runtime_protocol: 2,
                    controller_version: args[5].to_str().context("invalid controller version")?,
                },
            )?;
            println!(
                "{}",
                json!({"sealed":true,"manifest_sha256":result.sha256(),"key_sha256":result.signer_fingerprint()})
            );
        }
        _ => bail!(
            "usage: wukong-package key <absolute-controller-key-directory> | seal <package-directory> <external-manifest-draft> <absolute-controller-key-directory> <controller-version>"
        ),
    }
    Ok(())
}
