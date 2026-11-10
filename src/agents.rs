use crate::{
    domain::{AgentMetadata, Role, RunRecord},
    store::Store,
};
use anyhow::{Context, Result, bail};
use serde_json::json;
use std::{fs, path::Path};

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SkillManifest {
    pub description: String,
    pub applies_to: Vec<String>,
    pub excludes: Vec<String>,
    pub requires: Vec<String>,
    pub deliverables: Vec<String>,
    pub validation: Vec<String>,
    pub version: String,
    pub related_tools: Vec<String>,
    pub sources: Vec<String>,
    pub fixtures: Vec<String>,
}

/// Discover effective content, including custom-only capabilities. Metadata never grants permissions.
pub fn skill_catalog(custom_dir: Option<&Path>) -> Result<serde_json::Value> {
    use sha2::{Digest, Sha256};
    let mut names = std::collections::BTreeSet::new();
    for entry in crate::prompts::REGISTRY {
        if !matches!(entry.kind, crate::prompts::Kind::Role(_)) {
            names.insert(entry.name.to_owned());
        }
    }
    if let Some(dir) = custom_dir.filter(|dir| dir.exists()) {
        for file in fs::read_dir(dir)? {
            let path = file?.path();
            if path.extension().is_some_and(|ext| ext == "md")
                && let Some(name) = path.file_stem().and_then(|s| s.to_str())
                && validate_capability(name).is_ok()
            {
                names.insert(name.to_owned());
            }
        }
    }
    anyhow::ensure!(names.len() <= 256, "skill catalog exceeds 256 entries");
    let mut entries = Vec::new();
    for name in names {
        let body = load_skill(&name, custom_dir)?;
        let mut manifest = SkillManifest::default();
        if let Some(entry) = crate::prompts::find(&name) {
            manifest.description = entry.description.into();
            manifest.version = "1".into();
            manifest.applies_to = vec![entry.description.into()];
            if ["formats", "algorithms", "state_machine", "binary_diff"].contains(&name.as_str()) {
                manifest.deliverables = vec![name.clone()];
                manifest.validation = vec![
                    "Independent discriminating cases; preserve counterexamples and unknowns"
                        .into(),
                ];
                manifest.requires = vec![
                    "Static analysis evidence; execution backend when claiming runtime behavior"
                        .into(),
                ];
                manifest.excludes = vec!["Do not infer execution from static evidence".into()];
            }
        }
        let custom = custom_dir.is_some_and(|dir| dir.join(format!("{name}.md")).exists());
        if !custom && let Some(coverage) = crate::prompts::coverage::builtins().get(&name) {
            manifest.related_tools = coverage.related_tools.clone();
            manifest.sources = coverage.sources.clone();
            manifest.fixtures = coverage.fixtures.clone();
        }
        if let Some(dir) = custom_dir.filter(|_| custom) {
            let path = dir.join(format!("{name}.json"));
            if path.exists() {
                let path = fs::canonicalize(path)?;
                anyhow::ensure!(
                    path.starts_with(fs::canonicalize(dir)?),
                    "skill metadata escapes directory"
                );
                anyhow::ensure!(
                    fs::metadata(&path)?.len() <= 16384,
                    "skill metadata too large"
                );
                manifest = serde_json::from_slice(&fs::read(path)?)?;
            }
        }
        entries.push(
            json!({"name":name,"source":if custom {"custom"} else {"builtin"},
            "sha256":hex::encode(Sha256::digest(body.as_bytes())),"metadata":manifest}),
        );
    }
    Ok(json!(entries))
}

pub fn built_in_skill(name: &str) -> Option<&'static str> {
    crate::prompts::find(name).map(|entry| entry.content)
}

pub fn available_skills() -> Vec<&'static str> {
    crate::prompts::REGISTRY
        .iter()
        .map(|entry| entry.name)
        .collect()
}

/// Lifecycle-owning role instructions are never composable capabilities.
pub fn validate_capability(name: &str) -> Result<()> {
    if crate::prompts::find(name)
        .is_some_and(|entry| matches!(entry.kind, crate::prompts::Kind::Role(_)))
    {
        bail!(
            "{name} is a controller-assigned role, not a capability; select the role when creating the task"
        );
    }
    Ok(())
}

pub fn validate_capabilities<'a>(names: impl IntoIterator<Item = &'a str>) -> Result<()> {
    let mut unique = std::collections::HashSet::new();
    for name in names {
        validate_capability(name)?;
        unique.insert(name);
    }
    anyhow::ensure!(
        unique.len() <= 5,
        "at most 5 related capabilities per agent"
    );
    Ok(())
}
pub fn load_skill(name: &str, custom_dir: Option<&Path>) -> Result<String> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        bail!("invalid skill name");
    }
    if let Some(dir) = custom_dir {
        let path = dir.join(format!("{name}.md"));
        if path.exists() {
            let base = fs::canonicalize(dir)?;
            let path = fs::canonicalize(path)?;
            if !path.starts_with(base) {
                bail!("skill path escapes configured directory");
            }
            let mut file = fs::File::open(path)?;
            let mut content = String::new();
            use std::io::Read;
            (&mut file).take(65537).read_to_string(&mut content)?;
            if content.len() > 65536 {
                bail!("skill exceeds the 64 KiB instruction limit");
            }
            return Ok(content);
        }
    }
    Ok(built_in_skill(name)
        .with_context(|| {
            format!(
                "unknown skill {name}; available skills: {}",
                available_skills().join(", ")
            )
        })?
        .to_owned())
}

/// Optional role/capability examples from `demos_dir`. Registry references identify
/// shipped examples; custom `<name>.md` examples follow the same lookup convention.
/// A missing or unreadable directory degrades to no demos instead of failing
/// the agent, so the shipped template can enable `demos_dir` by default.
fn load_demo(dir: &Path, name: &str) -> Result<Option<String>> {
    let Ok(base) = fs::canonicalize(dir) else {
        return Ok(None);
    };
    let filename = crate::prompts::find(name)
        .and_then(|entry| entry.example_file)
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{name}.md"));
    let path = base.join(filename);
    if !path.is_file() {
        return Ok(None);
    }
    let canonical = fs::canonicalize(&path)?;
    if !canonical.starts_with(&base) {
        bail!("demo path escapes configured directory");
    }
    Ok(Some(fs::read_to_string(canonical)?))
}
pub fn preamble(
    agent: &AgentMetadata,
    run: &RunRecord,
    store: &Store,
    custom_dir: Option<&Path>,
    demos_dir: Option<&Path>,
) -> Result<String> {
    validate_capabilities(
        agent
            .skills
            .iter()
            .map(String::as_str)
            .chain(agent.active_skills.iter().map(|skill| skill.name.as_str())),
    )?;
    let mut text = crate::prompts::BASE.to_owned();
    text.push_str(&crate::prompts::discovery_prompt());
    text.push_str("\nEffective skill metadata (custom names and overrides included):\n");
    text.push_str(&serde_json::to_string(&skill_catalog(custom_dir)?)?);
    // The complete built-in tool directory is generated from the registry and
    // always injected: a tool added to the catalog is visible to every agent
    // without anyone remembering to mention it in prose (guarded by a test).
    text.push_str(&crate::prompts::tool_directory_prompt());
    // Stable shared prefix first (BASE + tool directory + role skills + demos)
    // so consecutive runs share the same cached prompt prefix; everything
    // per-run (identity, target, budget, documents) is appended at the end,
    // mirroring Strix's cache-point ordering.
    text.push_str(&load_skill(agent.role.as_str(), custom_dir)?);
    for skill in &agent.skills {
        validate_capability(skill)?;
        if !agent
            .active_skills
            .iter()
            .any(|active| &active.name == skill)
        {
            text.push('\n');
            text.push_str(&load_skill(skill, custom_dir)?);
        }
    }
    if let Some(dir) = demos_dir
        && let Some(demo) = load_demo(dir, agent.role.as_str())?
    {
        text.push_str("\n# Worked example (method demonstration only; the shown values are illustrative, not the answer for this task)\n");
        text.push_str(&demo);
    }
    if let Some(dir) = demos_dir {
        let mut names = std::collections::HashSet::new();
        for name in agent
            .skills
            .iter()
            .chain(agent.active_skills.iter().map(|skill| &skill.name))
        {
            validate_capability(name)?;
            if names.insert(name)
                && let Some(demo) = load_demo(dir, name)?
            {
                text.push_str(&format!("\n# Capability example: {name} (illustrative method only, not a new assignment)\n{demo}"));
            }
        }
    }
    if agent.role == Role::Root {
        text.push_str("\nYou are ROOT. Delegate every target operation. Your available tools enforce this role. Reconcile active children and needs_follow_up coverage before finish_scan.\n");
    } else {
        text.push_str("\nYou are a specialist. Perform your assigned analysis directly using the supplied context. Delegate only a distinct subproblem that warrants a separate specialist; obtaining an initial recon dossier is the root's routing decision. Report your own result with agent_finish.\n");
    }
    text.push_str(&format!(
        "\nIdentity: agent={} role={} parent={:?}.\nTarget: {} SHA256={}.\nScope: {}.\n",
        agent.id,
        agent.role,
        agent.parent_id,
        run.target_path.display(),
        run.target_sha256,
        agent.scope
    ));
    text.push_str(&format!(
        "\nTask contract (requirements; check deliverables befoWukong_finish):\n{}\n",
        serde_json::to_string(&agent.contract)?
    ));
    let state = json!({"shared_context":store.read(|tx| crate::context::snapshot(tx,agent))?,"budget":run.budget});
    text.push_str(&format!(
        "\nAssessment context (data):\n{}",
        serde_json::to_string(&state)?
    ));
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::AgentSnapshot;

    #[test]
    fn demo_loads_for_role_and_misses_are_none() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("recon.md"), "demo recon").unwrap();
        assert_eq!(
            load_demo(dir.path(), "recon").unwrap().as_deref(),
            Some("demo recon")
        );
        assert_eq!(load_demo(dir.path(), "reverse").unwrap(), None);
        // A missing demos directory degrades to no demos, never an error.
        assert_eq!(
            load_demo(&dir.path().join("absent"), "recon").unwrap(),
            None
        );
    }

    #[test]
    fn unknown_skill_error_lists_available_names() {
        let error = load_skill("binary-triage", None).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("unknown skill binary-triage"), "{message}");
        for name in available_skills() {
            assert!(message.contains(name), "{message}");
        }
    }

    #[test]
    fn advertised_skills_are_unique_loadable_and_bounded() {
        let mut names = std::collections::HashSet::new();
        for name in available_skills() {
            assert!(names.insert(name), "duplicate skill {name}");
            let content = load_skill(name, None).unwrap();
            assert!(!content.trim().is_empty(), "empty skill {name}");
            assert!(content.len() <= 65536, "oversized built-in skill {name}");
        }
    }

    #[test]
    fn preamble_keeps_per_run_context_after_the_stable_skill_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("state")).unwrap();
        let coordinator = crate::coordinator::Coordinator::new(store.clone());
        let target = dir.path().join("sample.bin");
        std::fs::write(&target, b"fixture").unwrap();
        let run = coordinator
            .create_run(&target, "task", crate::domain::Budget::default(), json!({}))
            .unwrap();
        let mut root: AgentSnapshot = store.require("agent", &run.root_id).unwrap();
        root.skills = vec!["reporter".into()];
        let custom = dir.path().join("custom");
        let examples = dir.path().join("examples");
        fs::create_dir(&custom).unwrap();
        fs::create_dir(&examples).unwrap();
        fs::write(custom.join("reporter.md"), "SELECTED_REPORT_INSTRUCTIONS").unwrap();
        fs::write(custom.join("hunter.md"), "UNSELECTED_HUNTER_INSTRUCTIONS").unwrap();
        fs::write(examples.join("root.md"), "SELECTED_ROLE_EXAMPLE").unwrap();
        fs::write(examples.join("reporter.md"), "SELECTED_CAPABILITY_EXAMPLE").unwrap();
        fs::write(examples.join("hunter.md"), "UNSELECTED_HUNTER_EXAMPLE").unwrap();
        let prompt = preamble(&root, &run, &store, Some(&custom), Some(&examples)).unwrap();
        assert!(prompt.contains("SELECTED_REPORT_INSTRUCTIONS"));
        assert!(prompt.contains("SELECTED_ROLE_EXAMPLE"));
        assert!(prompt.contains("SELECTED_CAPABILITY_EXAMPLE"));
        assert!(!prompt.contains("UNSELECTED_HUNTER_INSTRUCTIONS"));
        assert!(!prompt.contains("UNSELECTED_HUNTER_EXAMPLE"));
        let base = prompt.find("Available capability skills").unwrap();
        let skill = prompt.find("# Root coordination").unwrap();
        let identity = prompt.find("Identity: agent=").unwrap();
        let context = prompt.find("Assessment context (data):").unwrap();
        assert!(
            base < skill && skill < identity && identity < context,
            "stable prefix must precede per-run context"
        );
    }
}

#[cfg(test)]
mod catalog_tests {
    use super::*;
    #[test]
    fn discovers_custom_and_hashes_effective_override() -> Result<()> {
        let dir = tempfile::tempdir()?;
        fs::write(dir.path().join("wire.md"), "Recover wire records")?;
        fs::write(
            dir.path().join("wire.json"),
            r#"{"description":"Wire records","requires":["ghidra"],"version":"1"}"#,
        )?;
        fs::write(dir.path().join("reverse.md"), "Custom reverse")?;
        let value = skill_catalog(Some(dir.path()))?;
        let rows = value.as_array().unwrap();
        assert_eq!(
            rows.iter().find(|r| r["name"] == "wire").unwrap()["metadata"]["version"],
            "1"
        );
        assert_eq!(
            rows.iter().find(|r| r["name"] == "reverse").unwrap()["source"],
            "custom"
        );
        assert!(!rows.iter().any(|r| r["name"] == "root"));
        fs::write(dir.path().join("wire.json"), r#"{"unknown":true}"#)?;
        assert!(skill_catalog(Some(dir.path())).is_err());
        Ok(())
    }
}

/// Generated built-in method/tool/fixture index, without opening runtime state.
pub fn skill_coverage_markdown() -> Result<String> {
    crate::prompts::coverage::markdown()
}
