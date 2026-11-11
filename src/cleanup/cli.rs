use super::*;
use clap::Subcommand;
#[derive(Subcommand)]
pub enum Command {
    /// Open the shared uninstall scope review. Exit 32..39 confirms scope; 64 cancels.
    Choose {
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        initial_data: bool,
    },
    /// Open the installed platform uninstaller (Linux also removes managed integration).
    Uninstall,
    /// Preview exact application-owned paths. No resources are deleted.
    Plan {
        #[arg(long)]
        data: bool,
        #[arg(long)]
        tools: bool,
        #[arg(long)]
        challenges: bool,
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
        #[arg(long)]
        defer_finalize: bool,
    },
    /// Finalize a successfully deferred native uninstall.
    Finish {
        #[arg(long)]
        review: PathBuf,
    },
}
pub fn run(command: Command) -> Result<()> {
    let cleanup = Cleanup::current()?;
    match command {
        Command::Choose {
            output,
            initial_data,
        } => {
            #[cfg(feature = "desktop")]
            crate::desktop::cleanup::choose(output, initial_data)?;
            #[cfg(not(feature = "desktop"))]
            {
                let _ = (output, initial_data);
                anyhow::bail!("this build does not include the desktop review");
            }
        }
        Command::Uninstall => super::uninstall::run()?,
        Command::Finish { review } => cleanup.finish(&read_plan(&review)?.fingerprint)?,
        Command::Plan {
            data,
            tools,
            challenges,
            output,
        } => {
            let plan = cleanup.plan(Selection {
                data,
                tools,
                challenges,
            })?;
            if let Some(output) = output {
                check_output(&plan, &output)?;
                crate::workbench::write_json(&output, &plan)?;
            } else {
                println!("{}", serde_json::to_string_pretty(&plan)?);
            }
        }
        Command::Execute {
            review,
            yes,
            report,
            defer_finalize,
        } => {
            ensure!(yes, "execution requires explicit --yes");
            let reviewed = read_plan(&review)?;
            if let Some(output) = &report {
                check_output(&reviewed, output)?;
            }
            let result = if !reviewed.selection.any() {
                let mut errors = reviewed.errors.clone();
                if cleanup
                    .home
                    .join("updates/cleanup-in-progress.json")
                    .exists()
                    && errors.is_empty()
                {
                    errors.push("previous cleanup awaits finalization; select a cleanup scope and retry the uninstall".into());
                }
                Report {
                    complete: errors.is_empty(),
                    removed_files: 0,
                    removed_resources: 0,
                    removed_bytes: 0,
                    retained: vec![],
                    errors,
                }
            } else {
                cleanup
                    .execute_deferred(&reviewed, defer_finalize)
                    .unwrap_or_else(|error| Report {
                        complete: false,
                        removed_files: 0,
                        removed_resources: 0,
                        removed_bytes: 0,
                        retained: vec![],
                        errors: vec![error.to_string()],
                    })
            };
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
