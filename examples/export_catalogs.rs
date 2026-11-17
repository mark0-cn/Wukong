//! Regenerate the checked-in human catalogs from the built-in registries.
//!
//! `cargo run --example export_catalogs -- tool-catalog > docs/reference/tool-catalog.md`
use anyhow::Result;
use clap::{Parser, Subcommand};
use wukong::i18n::Locale;

#[derive(Parser)]
#[command(about = "Regenerate the checked-in tool and Skill catalogs")]
struct Arguments {
    /// Catalog language; English is reproducible by default.
    #[arg(long, value_parser = ["en-US", "zh-CN"], default_value = "en-US")]
    locale: String,
    /// Print the structured catalog instead of Markdown.
    #[arg(long)]
    json: bool,
    #[command(subcommand)]
    catalog: Catalog,
}

#[derive(Subcommand)]
enum Catalog {
    /// Print the canonical tool catalog.
    ToolCatalog,
    /// Print canonical Skill provenance and mechanism coverage.
    SkillCatalog,
}

fn main() -> Result<()> {
    let arguments = Arguments::parse();
    let locale = if arguments.locale == "zh-CN" {
        Locale::ZhCn
    } else {
        Locale::EnUs
    };
    match arguments.catalog {
        Catalog::ToolCatalog if arguments.json => print_json(&serde_json::to_value(
            wukong::tools::builtin_tool_definitions(),
        )?)?,
        Catalog::ToolCatalog => {
            print!(
                "{}",
                wukong::tools::builtin_catalog_markdown_for_locale(locale)?
            )
        }
        Catalog::SkillCatalog if arguments.json => {
            print_json(&wukong::agents::skill_catalog(None)?)?
        }
        Catalog::SkillCatalog => print!(
            "{}",
            wukong::agents::skill_coverage_markdown_for_locale(locale)?
        ),
    }
    Ok(())
}

fn print_json(value: &serde_json::Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
