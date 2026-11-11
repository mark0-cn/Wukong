//! Local-daemon, workspace-label scoped removal. Reuses the bounded Docker client.
use super::*;
use crate::workbench::{Docker, read_json};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Resource {
    pub kind: String,
    pub id: String,
    pub name: String,
    pub workspace: String,
    pub executable: String,
    pub binding: String,
    pub daemon: String,
}
pub(super) fn workspaces(home: &Path) -> Result<Vec<(PathBuf, String)>> {
    let registry = locations::inventory(home)?;
    let mut roots = BTreeSet::new();
    for location in registry.locations {
        if location.kind == LocationKind::Workbench {
            roots.insert(location.path);
        } else if location.kind == LocationKind::Cache {
            let probes = location.path.join("probes");
            if probes.is_dir() {
                files::safe(&probes)?;
                for entry in std::fs::read_dir(probes)?.take(1025) {
                    roots.insert(entry?.path());
                }
            }
        }
    }
    ensure!(roots.len() <= 2048, "too many workbench locations");
    let mut result = vec![];
    for root in roots {
        let path = root.join("workspace.json");
        if !path.is_file() {
            continue;
        }
        files::safe(&path)?;
        let value: Value = read_json(&path)?;
        ensure!(value["schema_version"] == 1, "invalid workspace identity");
        let id = value["id"].as_str().context("missing workspace ID")?;
        crate::workbench::valid_id(id)?;
        result.push((root, id.to_owned()));
    }
    Ok(result)
}
fn docker(home: &Path) -> Result<Docker> {
    let path = home.join("wukong.toml");
    let executable = if path.is_file() {
        files::safe(&path)?;
        let raw: toml::Value = toml::from_str(&std::fs::read_to_string(path)?)?;
        raw.get("runtime")
            .and_then(|v| v.get("docker_executable"))
            .and_then(toml::Value::as_str)
            .unwrap_or("docker")
            .to_owned()
    } else {
        "docker".into()
    };
    Ok(Docker::new(&executable))
}
fn local(docker: &Docker) -> Result<()> {
    // Explicit DOCKER_HOST overrides the current context, unless DOCKER_CONTEXT is set.
    let host = if std::env::var_os("DOCKER_CONTEXT").is_none()
        && let Ok(host) = std::env::var("DOCKER_HOST")
    {
        host
    } else {
        serde_json::from_str::<Vec<Value>>(&docker.run(&["context", "inspect"])?)?
            .first()
            .and_then(|v| v["Endpoints"]["docker"]["Host"].as_str())
            .context("cannot establish Docker daemon location")?
            .to_owned()
    };
    ensure!(
        host.starts_with("unix://") || host.starts_with("npipe://"),
        "cleanup only supports local Docker sockets; remote environments are retained"
    );
    Ok(())
}
fn label(value: &Value) -> Option<&str> {
    value
        .pointer("/Config/Labels/wukong.workspace")
        .or_else(|| value.pointer("/Labels/wukong.workspace"))
        .and_then(Value::as_str)
}
fn tool_name(kind: &str, name: &str, workspace: &str) -> bool {
    let suffix = match kind {
        "container" => "tools",
        "volume" => "state",
        _ => "offline",
    };
    name == format!("wukong-{workspace}-{suffix}")
}
fn challenge_ids(root: &Path) -> Result<BTreeSet<String>> {
    let directory = root.join("challenges");
    if !directory.is_dir() {
        return Ok(BTreeSet::new());
    }
    files::safe(&directory)?;
    let mut ids = BTreeSet::new();
    for entry in std::fs::read_dir(directory)?.take(4097) {
        let path = entry?.path().join("challenge.toml");
        if !path.is_file() {
            continue;
        }
        files::safe(&path)?;
        ensure!(
            std::fs::metadata(&path)?.len() <= 1024 * 1024,
            "challenge manifest too large"
        );
        let value: toml::Value = toml::from_str(&std::fs::read_to_string(&path)?)?;
        let id = value
            .get("id")
            .and_then(toml::Value::as_str)
            .context("challenge ID missing")?;
        crate::workbench::valid_id(id)?;
        ensure!(
            value
                .get("schema_version")
                .and_then(toml::Value::as_integer)
                == Some(1)
                && path
                    .parent()
                    .and_then(Path::file_name)
                    .and_then(|s| s.to_str())
                    == Some(id),
            "challenge identity changed"
        );
        ids.insert(id.to_owned());
    }
    ensure!(ids.len() <= 4096, "too many challenge manifests");
    Ok(ids)
}
fn challenge_name(kind: &str, name: &str, workspace: &str, ids: &BTreeSet<String>) -> bool {
    ids.iter().any(|id| {
        let prefix = format!("wukong-{workspace}-{id}-");
        let Some(suffix) = name.strip_prefix(&prefix) else {
            return false;
        };
        match kind {
            "container" => {
                matches!(suffix, "tools" | "service" | "debug")
                    || ["extract-", "pwninit-"].iter().any(|prefix| {
                        suffix.strip_prefix(prefix).is_some_and(|s| {
                            s.len() == 8 && s.bytes().all(|c| c.is_ascii_hexdigit())
                        })
                    })
            }
            "volume" => suffix == "state",
            "network" => matches!(suffix, "net" | "egress"),
            _ => false,
        }
    })
}
fn binding(kind: &str, value: &Value) -> Result<String> {
    // Exclude volatile counters/stats; bind identity, ownership and references.
    let fields = match kind {
        "container" => serde_json::json!([
            value["Id"],
            value["Name"],
            value["Created"],
            value["Config"]["Labels"],
            value["Image"],
            value["Mounts"]
        ]),
        "volume" => serde_json::json!([
            value["Name"],
            value["CreatedAt"],
            value["Driver"],
            value["Labels"],
            value["Options"]
        ]),
        "network" => serde_json::json!([
            value["Id"],
            value["Created"],
            value["Labels"],
            value["Driver"]
        ]),
        _ => serde_json::json!([
            value["Id"],
            value["RepoTags"],
            value["RepoDigests"],
            value["Config"]["Labels"]
        ]),
    };
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(&fields)?)))
}
fn resource(
    docker: &Docker,
    kind: &str,
    value: &Value,
    workspace: &str,
    daemon: &str,
) -> Result<Resource> {
    let id = value[if kind == "volume" { "Name" } else { "Id" }]
        .as_str()
        .context("resource ID missing")?
        .to_owned();
    let name = value["Name"]
        .as_str()
        .unwrap_or(&id)
        .trim_start_matches('/')
        .to_owned();
    Ok(Resource {
        kind: kind.into(),
        id,
        name,
        workspace: workspace.into(),
        executable: docker.executable().into(),
        binding: binding(kind, value)?,
        daemon: daemon.into(),
    })
}
pub(super) fn inventory(home: &Path, plan: &mut Plan) -> Result<()> {
    let workspaces = workspaces(home)?;
    if workspaces.is_empty() {
        return Ok(());
    }
    let docker = docker(home)?;
    local(&docker)?;
    let daemon = docker.run(&["info", "--format", "{{.ID}}"])?;
    ensure!(!daemon.is_empty(), "Docker daemon identity missing");
    let mut images = BTreeMap::new();
    for (root, workspace) in workspaces {
        let challenges = challenge_ids(&root)?;
        for kind in ["container", "volume", "network"] {
            let filter = format!("label=wukong.workspace={workspace}");
            let args = if kind == "container" {
                vec!["ps", "-aq", "--no-trunc", "--filter", &filter]
            } else {
                vec![kind, "ls", "-q", "--filter", &filter]
            };
            for id in docker.run(&args)?.lines() {
                let value = docker
                    .inspect(kind, id)?
                    .context("resource disappeared during inspection")?;
                ensure!(
                    label(&value) == Some(&workspace),
                    "Docker resource ownership changed"
                );
                let item = resource(&docker, kind, &value, &workspace, &daemon)?;
                let tool = tool_name(kind, &item.name, &workspace);
                if !tool && !challenge_name(kind, &item.name, &workspace, &challenges) {
                    plan.retained.push(Retained {
                        path: item.name,
                        reason: "unrecognized workspace resource name; retained".into(),
                    });
                    continue;
                }
                if !(if tool {
                    plan.selection.tools
                } else {
                    plan.selection.challenges
                }) {
                    plan.retained.push(Retained {
                        path: item.name,
                        reason: "Docker scope not selected".into(),
                    });
                    continue;
                }
                if kind == "container"
                    && let Some(image) = value["Image"].as_str()
                {
                    images.insert(image.to_owned(), workspace.clone());
                }
                plan.resources.push(item);
            }
        }
        if plan.selection.challenges {
            let filter = format!("label=wukong.workspace={workspace}");
            for id in docker
                .run(&["image", "ls", "-q", "--no-trunc", "--filter", &filter])?
                .lines()
            {
                images.insert(id.into(), workspace.clone());
            }
        }
    }
    // Tool images can survive after their container was removed. Only the public
    // product repository qualifies; unrelated retags/shared references are retained.
    if plan.selection.tools {
        for id in docker
            .run(&["image", "ls", "-q", "--no-trunc", "hlmark0/wukong"])?
            .lines()
        {
            images.entry(id.into()).or_default();
        }
    }
    for (id, workspace) in images {
        let Some(value) = docker.inspect("image", &id)? else {
            continue;
        };
        let official = value["RepoDigests"].as_array().is_some_and(|items| {
            items.iter().any(|v| {
                v.as_str()
                    .is_some_and(|s| s.starts_with("hlmark0/wukong@sha256:"))
            })
        });
        let foreign_tag = value["RepoTags"].as_array().is_some_and(|items| {
            items.iter().any(|v| {
                v.as_str()
                    .is_some_and(|s| !s.starts_with("hlmark0/wukong:"))
            })
        });
        if !(label(&value) == Some(&workspace) && !workspace.is_empty())
            && !(official && !foreign_tag && plan.selection.tools)
        {
            plan.retained.push(Retained {
                path: id,
                reason: "unverified or shared image ownership".into(),
            });
            continue;
        }
        plan.resources
            .push(resource(&docker, "image", &value, &workspace, &daemon)?);
    }
    plan.resources.sort_by_key(|item| {
        (
            match item.kind.as_str() {
                "container" => 0,
                "volume" => 1,
                "network" => 2,
                _ => 3,
            },
            item.id.clone(),
        )
    });
    plan.resources
        .dedup_by(|a, b| a.kind == b.kind && a.id == b.id);
    // Preview shared references instead of discovering these only after deletion.
    let selected: BTreeSet<_> = plan
        .resources
        .iter()
        .filter(|r| r.kind == "container")
        .map(|r| r.id.clone())
        .collect();
    let mut keep = vec![];
    for item in std::mem::take(&mut plan.resources) {
        if referenced(&docker, &item, &selected)? {
            plan.retained.push(Retained {
                path: item.name,
                reason: "referenced by a retained container".into(),
            });
        } else {
            keep.push(item);
        }
    }
    plan.resources = keep;
    Ok(())
}
fn referenced(docker: &Docker, item: &Resource, selected: &BTreeSet<String>) -> Result<bool> {
    if item.kind == "container" {
        return Ok(false);
    }
    let filter = format!(
        "{}={}",
        match item.kind.as_str() {
            "volume" => "volume",
            "network" => "network",
            _ => "ancestor",
        },
        item.id
    );
    Ok(docker
        .run(&["ps", "-aq", "--no-trunc", "--filter", &filter])?
        .lines()
        .any(|id| !selected.contains(id)))
}
pub(super) fn remove(resources: &[Resource], report: &mut Report) {
    for item in resources {
        let result = (|| -> Result<()> {
            let docker = Docker::new(&item.executable);
            local(&docker)?;
            ensure!(
                docker.run(&["info", "--format", "{{.ID}}"])? == item.daemon,
                "Docker daemon changed after review"
            );
            let Some(value) = docker.inspect(&item.kind, &item.id)? else {
                return Ok(());
            };
            ensure!(
                binding(&item.kind, &value)? == item.binding,
                "resource identity changed"
            );
            ensure!(
                !referenced(&docker, item, &BTreeSet::new())?,
                "resource gained a container reference"
            );
            if item.kind == "container" {
                ensure!(
                    label(&value) == Some(item.workspace.as_str()),
                    "container ownership changed"
                );
                if value["State"]["Running"] == true {
                    docker.run(&["stop", "--time", "10", &item.id])?;
                }
            }
            docker.run(&[&item.kind, "rm", &item.id])?;
            Ok(())
        })();
        match result {
            Ok(()) => report.removed_resources += 1,
            Err(error) => report
                .errors
                .push(format!("{} {}: {error}", item.kind, item.name)),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inherited_image_labels_do_not_adopt_manual_or_unknown_challenge_containers() {
        let ids = BTreeSet::from(["sample".to_owned()]);
        assert!(challenge_name(
            "container",
            "wukong-local-sample-service",
            "local",
            &ids
        ));
        for name in [
            "wukong-local-foreign",
            "wukong-local-other-service",
            "wukong-other-sample-service",
            "wukong-local-sample-extract-personal",
        ] {
            assert!(
                !challenge_name("container", name, "local", &ids),
                "adopted {name}"
            );
        }
        assert!(challenge_name(
            "container",
            "wukong-local-sample-extract-a12f8760",
            "local",
            &ids
        ));
        assert!(!challenge_name(
            "volume",
            "wukong-local-sample-net",
            "local",
            &ids
        ));
    }
}
