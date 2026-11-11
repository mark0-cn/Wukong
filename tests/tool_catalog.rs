use anyhow::Result;
use serde_json::Value;
use std::process::Command;

#[test]
fn generated_catalog_matches_the_executable_registry() {
    assert_eq!(
        wukong::tools::builtin_catalog_markdown(),
        include_str!("../docs/reference/tool-catalog.md")
    );
}

#[test]
fn chinese_catalogs_match_sources_and_preserve_registry_visibility() -> Result<()> {
    use wukong::i18n::Locale;
    let chinese = wukong::tools::builtin_catalog_markdown_for_locale(Locale::ZhCn)?;
    assert_eq!(
        chinese,
        include_str!("../docs/reference/tool-catalog.zh-CN.md")
    );
    assert_eq!(
        wukong::agents::skill_coverage_markdown_for_locale(Locale::ZhCn)?,
        include_str!("../docs/reference/skill-coverage.zh-CN.md")
    );
    for tool in wukong::tools::builtin_tool_definitions() {
        assert!(chinese.contains(&format!("| `{}` |", tool.name)));
    }
    Ok(())
}

#[test]
fn catalog_locale_changes_only_markdown_and_never_opens_state() -> Result<()> {
    let directory = tempfile::tempdir()?;
    for command in ["tool-catalog", "skill-catalog"] {
        let output = |locale: &str, json: bool| {
            let mut process = Command::new(env!("CARGO_BIN_EXE_wukong"));
            process
                .args(["--locale", locale, "--config"])
                .arg(directory.path().join("missing.toml"))
                .arg("--state")
                .arg(directory.path().join("must-not-exist"));
            if json {
                process.arg("--json");
            }
            let output = process.arg(command).output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            output.stdout
        };
        assert_eq!(output("en-US", true), output("zh-CN", true));
        assert_eq!(output("auto", false), output("en-US", false));
        assert_ne!(output("en-US", false), output("zh-CN", false));
    }
    assert!(!directory.path().join("must-not-exist").exists());
    Ok(())
}

#[test]
fn generated_skill_coverage_matches_registered_associations() -> Result<()> {
    assert_eq!(
        wukong::agents::skill_coverage_markdown()?,
        include_str!("../docs/reference/skill-coverage.md")
    );
    Ok(())
}

#[test]
fn skill_export_does_not_open_configuration_or_state() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let output = Command::new(env!("CARGO_BIN_EXE_wukong"))
        .args(["--json", "--config"])
        .arg(directory.path().join("missing.toml"))
        .arg("--state")
        .arg(directory.path().join("must-not-exist"))
        .arg("skill-catalog")
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let catalog: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(catalog, wukong::agents::skill_catalog(None)?);
    assert!(!directory.path().join("must-not-exist").exists());
    Ok(())
}

#[test]
fn catalog_export_does_not_open_configuration_or_state() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let output = Command::new(env!("CARGO_BIN_EXE_wukong"))
        .args(["--json", "--config"])
        .arg(directory.path().join("missing.toml"))
        .arg("--state")
        .arg(directory.path().join("must-not-exist"))
        .arg("tool-catalog")
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let definitions: Vec<Value> = serde_json::from_slice(&output.stdout)?;
    assert!(
        definitions
            .iter()
            .any(|definition| definition["name"] == "finish_scan")
    );
    assert!(
        definitions
            .iter()
            .any(|definition| definition["name"] == "agent_finish")
    );
    assert!(
        !definitions
            .iter()
            .any(|definition| definition["name"] == "create_agent")
    );
    assert!(!directory.path().join("must-not-exist").exists());
    Ok(())
}

#[test]
fn every_current_tool_has_a_display_label_and_unknown_tools_keep_their_name() {
    for tool in wukong::tools::builtin_tool_definitions() {
        let title = wukong::tools::tool_display_title(tool.name.as_str()).expect("missing label");
        assert!(!title.is_empty());
        assert_ne!(title, tool.name.as_str());
        for locale in [wukong::i18n::Locale::EnUs, wukong::i18n::Locale::ZhCn] {
            assert_ne!(locale.text(title), title, "missing localized tool label");
        }
    }
    assert!(wukong::tools::tool_display_title("mcp_custom_tool").is_none());
}
