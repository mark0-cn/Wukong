//! Operator entry point for the built-in offline Windows worker media finalizer.
//!
//! `wukong worker media generate` seals a released media template with the
//! controller signing identity, writes the finalized media directory, a
//! controller-credential-free ZIP and a generation record. It needs neither
//! Python nor a source checkout; the operation itself is Windows-only.
use anyhow::Result;
use clap::Subcommand;
use serde_json::Value;
use std::{net::Ipv4Addr, path::PathBuf};

#[derive(Subcommand)]
pub enum Command {
    /// Build finalized offline Windows worker media from a released template.
    Media {
        #[command(subcommand)]
        command: MediaCommand,
    },
}

#[derive(Subcommand)]
pub enum MediaCommand {
    /// Seal a template into media, a credential-free ZIP and a generation record.
    Generate {
        /// Released media template ZIP archive or unpacked template directory.
        #[arg(long)]
        template: PathBuf,
        /// New controller-credential-free media ZIP to write.
        #[arg(long)]
        output: PathBuf,
        /// Directory receiving the generation record, SSH identity and media.
        #[arg(long)]
        registry: PathBuf,
        /// Absolute controller signing key directory (created if absent).
        #[arg(long)]
        keys: PathBuf,
        /// Enrollment environment id (default: windows).
        #[arg(long)]
        environment_id: Option<String>,
        /// Worker management peer IPv4 address (default: 192.168.199.1).
        #[arg(long)]
        management_peer: Option<Ipv4Addr>,
        /// Fixed SSH port; omitted selects a free-ish port in 22220..=22299.
        #[arg(long)]
        ssh_port: Option<u16>,
        /// Media/controller version; defaults to the template version.
        #[arg(long)]
        controller_version: Option<String>,
    },
}

pub fn run(command: Command) -> Result<Value> {
    match command {
        Command::Media { command } => media(command),
    }
}

fn media(command: MediaCommand) -> Result<Value> {
    match command {
        MediaCommand::Generate {
            template,
            output,
            registry,
            keys,
            environment_id,
            management_peer,
            ssh_port,
            controller_version,
        } => generate(GenerateArgs {
            template,
            output,
            registry,
            keys,
            environment_id,
            management_peer,
            ssh_port,
            controller_version,
        }),
    }
}

#[derive(Clone, Debug)]
pub struct GenerateArgs {
    pub template: PathBuf,
    pub output: PathBuf,
    pub registry: PathBuf,
    pub keys: PathBuf,
    pub environment_id: Option<String>,
    pub management_peer: Option<Ipv4Addr>,
    pub ssh_port: Option<u16>,
    pub controller_version: Option<String>,
}

#[cfg(windows)]
fn generate(args: GenerateArgs) -> Result<Value> {
    use anyhow::Context;
    use std::path::absolute;
    let resolve = |path: PathBuf, label: &str| -> Result<PathBuf> {
        absolute(&path).with_context(|| format!("resolve offline media {label} {}", path.display()))
    };
    let request = crate::application::worker_media::GenerateRequest {
        template: resolve(args.template, "template")?,
        output_zip: resolve(args.output, "archive")?,
        registry: resolve(args.registry, "registry")?,
        keys: resolve(args.keys, "controller key directory")?,
        environment_id: args.environment_id,
        management_peer: args.management_peer,
        ssh_port: args.ssh_port,
        controller_version: args.controller_version,
        installation_id: None,
    };
    Ok(serde_json::to_value(
        crate::application::worker_media::generate(&request)?,
    )?)
}

#[cfg(not(windows))]
fn generate(_args: GenerateArgs) -> Result<Value> {
    anyhow::bail!("offline worker media generation requires a Windows controller")
}
