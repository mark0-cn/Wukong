//! Native worker operator entry points. Analysis requests never select package trust.
use anyhow::{Context, Result, ensure};
use serde_json::json;
use std::path::Path;
use wukong_worker_package::{Compatibility, PublicKey, verify_package};

fn main() -> Result<()> {
    ensure!(cfg!(windows), "native Windows worker requires Windows");
    let args: Vec<_> = std::env::args_os().collect();
    #[cfg(windows)]
    if args.len() == 5 && args[1] == "resource-execute" {
        let result = wukong_windows_worker::resource_process::run(
            args[2]
                .to_str()
                .context("invalid native process installation")?,
            Path::new(&args[3]),
            Path::new(&args[4]),
        )?;
        println!("{result}");
        return Ok(());
    }
    #[cfg(windows)]
    if args.len() == 5 && args[1] == "resource-selftest" {
        let report = wukong_windows_worker::resource_host::selftest(
            args[2].to_str().context("invalid resource installation")?,
            Path::new(&args[3]),
            args[4].to_str().context("invalid qualification identity")?,
        )?;
        println!("{report}");
        return Ok(());
    }
    #[cfg(windows)]
    if args.len() == 8 && args[1] == "isolation-probe" {
        let report = wukong_windows_worker::resource_host::probe(
            Path::new(&args[2]),
            Path::new(&args[3]),
            args[4].to_str().context("invalid probe port")?.parse()?,
            args[5].to_str().context("invalid probe SID")?,
            args[6]
                .to_str()
                .context("invalid fixed host PID")?
                .parse()?,
            args[7]
                .to_str()
                .context("invalid fixed host TID")?
                .parse()?,
        )?;
        println!("{report}");
        return Ok(());
    }
    #[cfg(windows)]
    if args.len() == 1 {
        let result = wukong_windows_worker::setup_ui::review();
        if let Err(error) = &result {
            wukong_windows_worker::setup_ui::failure(error);
        }
        return result;
    }
    #[cfg(windows)]
    if args.len() == 5 && args[1] == "install-elevated" {
        return wukong_windows_worker::setup_ui::elevated(
            Path::new(&args[2]),
            &PublicKey::from_hex(args[3].to_str().context("invalid controller key")?)?,
            args[4]
                .to_str()
                .context("invalid reviewed enrollment hash")?,
        );
    }
    #[cfg(windows)]
    if args.len() == 5 && args[1] == "install-reviewed" {
        let result = wukong_windows_worker::setup_ui::install_reviewed(
            Path::new(&args[2]),
            &PublicKey::from_hex(args[3].to_str().context("invalid pinned controller key")?)?,
            args[4]
                .to_str()
                .context("invalid reviewed enrollment hash")?,
        )?;
        println!("{}", result);
        return Ok(());
    }
    #[cfg(windows)]
    if args.len() == 6 && args[1] == "verify-enrollment" {
        let key = PublicKey::from_hex(args[4].to_str().context("invalid controller key")?)?;
        let controller = args[5].to_str().context("invalid controller version")?;
        let package = verify_package(
            Path::new(&args[2]),
            &key,
            &Compatibility {
                runtime_protocol: 2,
                controller_version: controller,
            },
        )?;
        let enrollment =
            wukong_windows_worker::deployment::read_enrollment(Path::new(&args[3]), &key)?;
        enrollment.bind_payload(&package)?;
        println!(
            "{}",
            json!({"verified":true,"scope":"public_enrollment_and_payload","installation_id":enrollment.enrollment().installation_id,"generation":enrollment.enrollment().generation,"manifest_sha256":package.sha256()})
        );
        return Ok(());
    }
    #[cfg(windows)]
    if args.len() == 4 && args[1] == "service" {
        return wukong_windows_worker::service::dispatch(
            args[2].to_str().context("invalid installation ID")?,
            Path::new(&args[3]),
        );
    }
    #[cfg(windows)]
    if args.len() == 4 && args[1] == "broker-health" {
        let installation = args[2].to_str().context("invalid broker installation ID")?;
        let parent = args[3]
            .to_str()
            .context("invalid broker parent PID")?
            .parse::<u32>()?;
        let reply = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(wukong_windows_worker::broker::health(installation, parent))?;
        println!("{reply}");
        return Ok(());
    }
    #[cfg(windows)]
    if args.len() == 3 && args[1] == "relay" {
        let installation = args[2].to_str().context("invalid installation ID")?;
        return tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(wukong_windows_worker::service::relay(installation));
    }
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
