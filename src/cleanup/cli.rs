use super::*;
use clap::Subcommand;
#[derive(Subcommand)]
pub enum Command {
    /// Preview exact application-owned paths. No resources are deleted.
    Plan {
        #[arg(long)]
        data: bool,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Execute a previously reviewed inventory after checking it is still current.
    Execute {
        #[arg(long)]
        review: PathBuf,
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        report: Option<PathBuf>,
    },
}
pub fn run(command: Command) -> Result<()> {
    let cleanup = Cleanup::current()?;
    match command {
        Command::Plan { data, output } => {
            let plan = cleanup.plan(Selection {
                data,
                ..Default::default()
            })?;
            if let Some(output) = output {
                crate::workbench::write_json(&output, &plan)?;
            } else {
                println!("{}", serde_json::to_string_pretty(&plan)?);
            }
        }
        Command::Execute {
            review,
            yes,
            report,
        } => {
            ensure!(yes, "execution requires explicit --yes");
            let result = cleanup.execute(&read_plan(&review)?)?;
            if let Some(report) = report {
                crate::workbench::write_json(&report, &result)?;
            }
            println!("{}", serde_json::to_string_pretty(&result)?);
            ensure!(
                result.complete,
                "cleanup incomplete; inspect the report before retrying"
            );
        }
    }
    Ok(())
}
