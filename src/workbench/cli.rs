use super::*;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum EnvCommand {
    Export {
        destination: PathBuf,
    },
    Import {
        source: PathBuf,
    },
    Up {
        #[arg(long)]
        image: Option<String>,
    },
    Status,
    Doctor,
    Down,
    Update {
        #[arg(long)]
        image: String,
    },
    Rollback,
}
pub fn environment(config: &AppConfig, command: EnvCommand) -> Result<Value> {
    let workbench = Workbench::open(config)?;
    let _lock = workbench.lock()?;
    match command {
        EnvCommand::Export { destination } => workbench.export_bundle(&destination),
        EnvCommand::Import { source } => workbench.import_bundle(&source),
        EnvCommand::Up { image } => workbench.up(image.as_deref()),
        EnvCommand::Status => workbench.status(),
        EnvCommand::Doctor => workbench.doctor(),
        EnvCommand::Down => workbench.down(),
        EnvCommand::Update { image } => workbench.update(&image),
        EnvCommand::Rollback => {
            let previous: super::environment::Environment =
                read_json(&workbench.root.join("previous-environment.json"))?;
            workbench.update(&previous.image_id)
        }
    }
}

#[derive(Subcommand)]
pub enum ChallengeCommand {
    Debug {
        id: String,
    },
    Pwninit {
        id: String,
    },
    Extract {
        id: String,
        #[arg(long)]
        path: String,
        #[arg(long)]
        role: String,
    },
    Prepare {
        id: String,
    },
    Build {
        id: String,
    },
    Up {
        id: String,
        #[arg(long)]
        allow_privileged: bool,
    },
    Down {
        id: String,
    },
    Status {
        id: String,
    },
    ExportRootfs {
        id: String,
    },
    Import {
        source: PathBuf,
        #[arg(long)]
        id: String,
        #[arg(long)]
        binary: Option<String>,
    },
    List,
    Show {
        id: String,
    },
    Verify {
        id: String,
    },
}
pub fn challenge(config: &AppConfig, command: ChallengeCommand) -> Result<Value> {
    let workbench = Workbench::open(config)?;
    let _lock = workbench.lock()?;
    match command {
        ChallengeCommand::Debug { id } => workbench.debug_challenge(&id),
        ChallengeCommand::Pwninit { id } => workbench.pwninit(&id),
        ChallengeCommand::Extract { id, path, role } => workbench.extract_file(&id, &path, &role),
        ChallengeCommand::Prepare { id } => workbench.prepare_challenge(&id),
        ChallengeCommand::Build { id } => workbench.build_challenge(&id),
        ChallengeCommand::Up {
            id,
            allow_privileged,
        } => workbench.up_challenge(&id, allow_privileged),
        ChallengeCommand::Down { id } => workbench.down_challenge(&id),
        ChallengeCommand::Status { id } => workbench.challenge_status(&id),
        ChallengeCommand::ExportRootfs { id } => workbench.export_rootfs(&id),
        ChallengeCommand::Import { source, id, binary } => {
            workbench.import_challenge(&source, &id, binary.as_deref())
        }
        ChallengeCommand::List => workbench.challenges(),
        ChallengeCommand::Show { id } => Ok(
            json!({"challenge":workbench.challenge(&id)?,"directory":workbench.challenge_dir(&id)?}),
        ),
        ChallengeCommand::Verify { id } => workbench.verify_original(&id),
    }
}
