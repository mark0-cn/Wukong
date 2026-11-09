use super::*;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum EnvCommand {
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
