//! Localized help is built from the derived command tree; option and command IDs stay fixed.
use super::Cli;
use crate::i18n::Locale;
use clap::{CommandFactory, FromArgMatches};
impl Cli {
    pub fn resolved_locale(&self) -> Locale {
        if self.locale == "auto" {
            Locale::system()
        } else {
            Locale::negotiate(&self.locale)
        }
    }
    pub fn parse_localized() -> Self {
        let args: Vec<_> = std::env::args_os().collect();
        let mut requested = std::env::var("WUKONG_LOCALE").unwrap_or_else(|_| "auto".into());
        for (i, arg) in args.iter().enumerate().take_while(|(_, arg)| *arg != "--") {
            if arg == "--locale" {
                if let Some(next) = args.get(i + 1) {
                    requested = next.to_string_lossy().into();
                }
            } else if let Some(value) = arg
                .to_str()
                .and_then(|value| value.strip_prefix("--locale="))
            {
                requested = value.into();
            }
        }
        let locale = if requested == "auto" {
            Locale::system()
        } else {
            Locale::negotiate(&requested)
        };
        let mut command = localize(Self::command(), locale, "");
        let matches = command
            .clone()
            .try_get_matches_from(args)
            .unwrap_or_else(|error| {
                if matches!(
                    error.kind(),
                    clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
                ) {
                    error.exit();
                }
                eprintln!("{}\n{error}", locale.text("cli-invalid-arguments"));
                std::process::exit(error.exit_code());
            });
        Self::from_arg_matches(&matches).unwrap_or_else(|error| error.format(&mut command).exit())
    }
}
fn localize(mut command: clap::Command, locale: Locale, parent: &str) -> clap::Command {
    let name = command.get_name().to_owned();
    let key = if parent.is_empty() {
        format!("cli-command-{name}")
    } else {
        format!("cli-command-{parent}-{name}")
    };
    if locale.has(&key) {
        command = command.about(locale.text(&key));
    }
    // Never mutate a built command: clap caches argument indexes during build.
    command = command
        .subcommand_help_heading(locale.text("cli-commands"))
        .disable_help_flag(true)
        .disable_version_flag(true)
        .arg(
            clap::Arg::new("help")
                .long("help")
                .short('h')
                .action(clap::ArgAction::Help),
        );
    if command.get_version().is_some() {
        command = command.arg(
            clap::Arg::new("version")
                .long("version")
                .short('V')
                .action(clap::ArgAction::Version),
        );
    }
    let arguments: Vec<_> = command
        .get_arguments()
        .map(|arg| (arg.get_id().to_string(), arg.is_positional()))
        .collect();
    for (id, positional) in arguments {
        let key = format!("cli-arg-{}", id.replace('_', "-"));
        if locale.has(&key) {
            command = command.mut_arg(id, |arg| {
                arg.help(locale.text(&key))
                    .help_heading(locale.text(if positional {
                        "cli-arguments"
                    } else {
                        "cli-options"
                    }))
            });
        }
    }
    if name == "analyze" && command.get_arguments().any(|arg| arg.get_id() == "task") {
        command = command.mut_arg("task", |arg| {
            arg.default_value(locale.text("cli-default-task"))
        });
    }
    command = command.help_template(format!(
        "{{about-with-newline}}\n{}: {{usage}}\n\n{{all-args}}{{after-help}}",
        locale.text("cli-usage")
    ));
    let children: Vec<_> = command
        .get_subcommands()
        .map(|sub| sub.get_name().to_owned())
        .collect();
    for child in children {
        command = command.mut_subcommand(child, |sub| {
            localize(sub, locale, if name == "wukong" { "" } else { &name })
        });
    }
    command
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn help_localizes_without_changing_command_ids() {
        let mut command = localize(Cli::command(), Locale::ZhCn, "");
        let help = command.render_long_help().to_string();
        assert!(help.contains("用法"));
        assert!(help.contains("--locale"));
        assert!(help.contains("analyze"));
        let matches = command
            .try_get_matches_from([
                "wukong",
                "--locale",
                "zh-CN",
                "--output-language",
                "en-US",
                "analyze",
                "sample",
            ])
            .unwrap();
        let cli = Cli::from_arg_matches(&matches).unwrap();
        assert_eq!(cli.output_language, Some(Locale::EnUs));
        assert_eq!(cli.resolved_locale(), Locale::ZhCn);
        let command = localize(Cli::command(), Locale::EnUs, "");
        let matches = command
            .try_get_matches_from([
                "wukong",
                "--config",
                "config.toml",
                "analyze",
                "target.bin",
                "--task",
                "Inspect only the test fixture",
                "--output",
                "report-output",
            ])
            .unwrap();
        let cli = Cli::from_arg_matches(&matches).unwrap();
        let super::super::Command::Analyze {
            target,
            task,
            output,
        } = cli.command.unwrap()
        else {
            panic!("wrong command");
        };
        assert_eq!(target, std::path::PathBuf::from("target.bin"));
        assert_eq!(task, "Inspect only the test fixture");
        assert_eq!(output, Some(std::path::PathBuf::from("report-output")));
    }
}
