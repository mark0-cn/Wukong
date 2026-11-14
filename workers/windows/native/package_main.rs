//! Controller operator utility. This binary is never part of VM runtime payloads.
use anyhow::{Context, Result, bail, ensure};
use serde_json::json;
use std::{fs, io::Read, path::Path};
use wukong_worker_package::{
    Compatibility, Enrollment, Manifest, load_controller_signer, seal_package, verify_package,
    write_enrollment,
};

fn outside_package(root: &Path, path: &Path) -> Result<()> {
    let destination = if path.try_exists()? {
        path.canonicalize()?
    } else {
        path.parent()
            .context("missing external output parent")?
            .canonicalize()?
            .join(path.file_name().context("missing external output name")?)
    };
    ensure!(
        !destination.starts_with(root),
        "controller keys, drafts and enrollment output must stay outside package"
    );
    Ok(())
}
fn read_draft(path: &Path, maximum: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(maximum as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= maximum, "operator draft exceeds size limit");
    Ok(bytes)
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().collect();
    match (args.get(1).and_then(|value| value.to_str()), args.len()) {
        (Some("inspect-pe"), 4) => {
            let architecture = args[3].to_str().context("invalid PE architecture")?;
            let imports = wukong_windows_worker::pe_imports::imported_libraries(
                Path::new(&args[2]),
                architecture,
            )?;
            println!(
                "{}",
                json!({"schema":1,"architecture":architecture,"imported_libraries":imports,"source":"object_pe"})
            );
        }
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
            outside_package(&root, keys)?;
            outside_package(&root, &draft)?;
            let bytes = read_draft(&draft, 4 * 1024 * 1024)?;
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
        (Some("enroll"), 7) => {
            let root_path = Path::new(&args[2]);
            let root = root_path.canonicalize()?;
            let draft = Path::new(&args[3]).canonicalize()?;
            let keys = Path::new(&args[4]);
            let output = Path::new(&args[5]);
            ensure!(
                keys.is_absolute() && output.is_absolute(),
                "controller key and enrollment output directories must be absolute"
            );
            for path in [draft.as_path(), keys, output] {
                outside_package(&root, path)?;
            }
            let enrollment: Enrollment = serde_json::from_slice(&read_draft(&draft, 16 * 1024)?)?;
            let signer = load_controller_signer(keys)?;
            let payload = verify_package(
                root_path,
                &signer.public_key(),
                &Compatibility {
                    runtime_protocol: 2,
                    controller_version: args[6].to_str().context("invalid controller version")?,
                },
            )?;
            let result = write_enrollment(output, &enrollment, &signer, &payload)?;
            println!(
                "{}",
                json!({"enrollment_prepared":true,"installation_id":result.enrollment().installation_id,
                "ssh_key_fingerprint":result.enrollment().ssh_public_key.fingerprint()?})
            );
        }
        _ => bail!(
            "usage: wukong-package inspect-pe <bootstrap-image> <x64|x86> | key <absolute-controller-key-directory> | seal <package-directory> <external-manifest-draft> <absolute-controller-key-directory> <controller-version> | enroll <package-directory> <external-enrollment-draft> <absolute-controller-key-directory> <new-absolute-enrollment-output> <controller-version>"
        ),
    }
    Ok(())
}
