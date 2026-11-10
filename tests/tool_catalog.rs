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
