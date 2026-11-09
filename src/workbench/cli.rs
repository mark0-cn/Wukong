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
