//! Native worker operator entry points. Analysis requests never select package trust.
use anyhow::{Context, Result, ensure};
use serde_json::json;
use std::path::Path;
use wukong_worker_package::{Compatibility, PublicKey, verify_package};

fn main() -> Result<()> {
    ensure!(cfg!(windows), "native Windows worker requires Windows");
    let args: Vec<_> = std::env::args_os().collect();
    ensure!(
        args.len() == 5 && args[1] == "verify-package",
        "usage: wukong-worker verify-package <unpacked-directory> <pinned-public-key-hex> <controller-version>"
    );
    let key = PublicKey::from_hex(args[3].to_str().context("invalid public key encoding")?)?;
    let controller = args[4]
        .to_str()
        .context("invalid controller version encoding")?;
    let package = verify_package(
        Path::new(&args[2]),
        &key,
        &Compatibility {
            runtime_protocol: 2,
            controller_version: controller,
        },
    )?;
    println!(
        "{}",
        json!({"verified":true,"scope":"signature_and_file_integrity","manifest_sha256":package.sha256(),"signer_sha256":package.signer_fingerprint(),
        "version":package.manifest().version,"runtime_protocol":package.manifest().runtime_protocol,
        "files":package.manifest().files.len()})
    );
    Ok(())
}
