//! Built-in prompt metadata. Names, discovery, bodies and example references
//! are maintained together; only selected bodies are installed in a prompt.
use crate::domain::Role;

pub(crate) const BASE: &str = include_str!("../prompts/base.md");
pub(crate) const EXAMPLES_DIR: &str = "prompts/examples";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Role(Role),
    Capability,
    Method,
    Support,
}

pub(crate) struct Entry {
    pub name: &'static str,
    pub kind: Kind,
    pub description: &'static str,
    pub content: &'static str,
    pub example_file: Option<&'static str>,
}

macro_rules! entry {
    ($name:ident, $kind:expr, $folder:literal, $description:literal) => {
        Entry {
            name: stringify!($name),
            kind: $kind,
            description: $description,
            content: include_str!(concat!(
                "../prompts/",
                $folder,
                "/",
                stringify!($name),
                ".md"
            )),
            example_file: None,
        }
    };
    ($name:ident, $kind:expr, $folder:literal, $description:literal, example) => {
        Entry {
            example_file: Some(concat!(stringify!($name), ".md")),
            ..entry!($name, $kind, $folder, $description)
        }
    };
}

pub(crate) static REGISTRY: &[Entry] = &[
    entry!(
        root,
        Kind::Role(Role::Root),
        "roles",
        "Coordinate research, reconcile evidence and own final delivery.",
        example
    ),
    entry!(
        analyst,
        Kind::Role(Role::Analyst),
        "roles",
        "Answer an assigned reverse-engineering question with evidence.",
        example
    ),
    entry!(
        verifier,
        Kind::Role(Role::Verifier),
        "roles",
        "Independently check another agent's claim.",
        example
    ),
    entry!(
        recon,
        Kind::Capability,
        "capabilities",
        "Establish missing target identity, input paths and analysis entry points.",
        example
    ),
    entry!(
        reverse,
        Kind::Capability,
        "capabilities",
        "Recover behavior, control flow and data flow for the assigned question.",
        example
    ),
    entry!(
        hunter,
        Kind::Capability,
        "capabilities",
        "Discover and test vulnerability hypotheses when security research is requested.",
        example
    ),
    entry!(
        fuzz,
        Kind::Capability,
        "capabilities",
        "Run a bounded input-generation campaign and preserve real artifacts."
    ),
    entry!(
        crash,
        Kind::Capability,
        "capabilities",
        "Reproduce, minimize and investigate a crash's root cause."
    ),
    entry!(
        exploit,
        Kind::Capability,
        "capabilities",
        "Establish a requested exploit primitive or minimal PoC from a confirmed cause.",
        example
    ),
    entry!(
        reporter,
        Kind::Capability,
        "capabilities",
        "Assemble a traceable report from existing analysis and verification evidence.",
        example
    ),
    entry!(
        formats,
        Kind::Method,
        "methods",
        "Recover file, packet and record layouts with discriminating samples."
    ),
    entry!(
        types,
        Kind::Method,
        "methods",
        "Recover object and structure layouts from access constraints."
    ),
    entry!(
        algorithms,
        Kind::Method,
        "methods",
        "Reconstruct transformation semantics and discriminating test vectors."
    ),
    entry!(
        state_machine,
        Kind::Method,
        "methods",
        "Recover state transitions, guards and reset behavior."
    ),
    entry!(
        binary_diff,
        Kind::Method,
        "methods",
        "Explain version differences using comparable evidence from both sides."
    ),
    entry!(
        heap_ptmalloc,
        Kind::Method,
        "methods",
        "Analyze glibc heap lifetimes and primitives using actual allocator identity and version-specific checks."
    ),
    entry!(
        pwn,
        Kind::Method,
        "methods",
        "Build a requested control-flow payload using confirmed layouts and runtime checks."
    ),
    entry!(
        tooling,
        Kind::Support,
        "support",
        "Use the configured worker's execution, solving and analysis toolchain."
    ),
    entry!(
        recovery,
        Kind::Support,
        "support",
        "Reconcile uncertain tool outcomes and resume the original assignment."
    ),
];

pub(crate) fn find(name: &str) -> Option<&'static Entry> {
    REGISTRY.iter().find(|entry| entry.name == name)
}

/// The complete built-in tool directory, generated from the executable
/// registry: every tool name with its first-sentence description. Injected
/// into every preamble so newly added tools are visible to agents without
/// prose updates; a test asserts the directory covers the full registry.
pub(crate) fn tool_directory_prompt() -> String {
    let mut definitions = crate::tools::builtin_tool_definitions();
    definitions.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
    let mut text = String::from(
        "\n\nTool directory (built-in purposes; only role-permitted schemas may be called):\n",
    );
    for definition in definitions {
        let mut short = definition
            .description
            .split(". ")
            .next()
            .unwrap_or_default()
            .trim()
            .to_owned();
        if short.chars().count() > 110 {
            short = short.chars().take(107).collect::<String>() + "…";
        }
        text.push_str(&format!("- {}: {}\n", definition.name, short));
    }
    text
}

pub(crate) fn discovery_prompt() -> String {
    let roles = REGISTRY
        .iter()
        .filter_map(|entry| match entry.kind {
            Kind::Role(role) => Some(role.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(", ");
    let mut text = format!(
        "\nController-assigned roles (not loadable capabilities): {roles}.\n\nAvailable capability skills (load by exact name with load_skill):\n"
    );
    for (kind, title) in [
        (Kind::Capability, "Capabilities"),
        (Kind::Method, "Task methods"),
        (Kind::Support, "Support"),
    ] {
        text.push_str(&format!("\n{title}:\n"));
        for entry in REGISTRY.iter().filter(|entry| entry.kind == kind) {
            text.push_str(&format!("- {}: {}\n", entry.name, entry.description));
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::BTreeSet, fs, path::Path};

    #[test]
    fn tool_directory_covers_the_full_registry() {
        // Structural guarantee: every built-in tool is advertised to agents
        // through the generated directory, so adding a tool can never be
        // "forgotten" at the prompt layer.
        let directory = tool_directory_prompt();
        for definition in crate::tools::builtin_tool_definitions() {
            assert!(
                directory.contains(definition.name.as_str()),
                "tool {} is missing from the generated tool directory",
                definition.name
            );
        }
    }

    #[test]
    fn prompt_resources_and_examples_have_registry_owners() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("prompts");
        for (directory, kind) in [
            ("roles", None),
            ("capabilities", Some(Kind::Capability)),
            ("methods", Some(Kind::Method)),
            ("support", Some(Kind::Support)),
        ] {
            let files = markdown_names(&root.join(directory));
            let registered = REGISTRY
                .iter()
                .filter(|entry| match kind {
                    Some(kind) => entry.kind == kind,
                    None => matches!(entry.kind, Kind::Role(_)),
                })
                .map(|entry| entry.name.to_owned())
                .collect::<BTreeSet<_>>();
            assert_eq!(
                files, registered,
                "unregistered or missing resource in {directory}"
            );
        }
        let examples = REGISTRY
            .iter()
            .filter_map(|entry| entry.example_file)
            .map(|name| {
                Path::new(name)
                    .file_stem()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_owned()
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(markdown_names(&root.join("examples")), examples);
        for entry in REGISTRY {
            assert!(
                !entry.description.trim().is_empty(),
                "missing purpose for {}",
                entry.name
            );
            if let Kind::Role(role) = entry.kind {
                assert_eq!(entry.name.parse::<Role>().unwrap(), role);
                assert!(crate::agents::validate_capability(entry.name).is_err());
            } else {
                crate::agents::validate_capability(entry.name).unwrap();
            }
            if let Some(example) = entry.example_file {
                assert!(
                    !fs::read_to_string(root.join("examples").join(example))
                        .unwrap()
                        .trim()
                        .is_empty()
                );
            }
        }
    }

    fn markdown_names(directory: &Path) -> BTreeSet<String> {
        fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "md"))
            .map(|path| path.file_stem().unwrap().to_str().unwrap().to_owned())
            .collect()
    }
}
