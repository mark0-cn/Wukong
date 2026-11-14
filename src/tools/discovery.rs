//! Role permissions intersect observed backend capabilities before each model request.
use super::*;
use std::collections::BTreeSet;

#[derive(Clone, Debug)]
pub(crate) struct ToolSelection {
    pub(crate) names: BTreeSet<String>,
    pub(crate) environment: Value,
}

impl ToolService {
    /// Initial access is conservative and performs no worker IO.
    pub(crate) fn model_tool_names(&self, agent: &AgentMetadata) -> Result<BTreeSet<String>> {
        Ok(self.selection(agent, None).names)
    }

    fn selection(&self, agent: &AgentMetadata, observation: Option<&Value>) -> ToolSelection {
        let local = self.local_ghidra().is_some_and(|path| path.is_file());
        let mut names = BTreeSet::new();
        let mut unavailable = Vec::new();
        for spec in builtins()
            .iter()
            .filter(|spec| spec.model_visible && spec.allows(agent.role))
        {
            if spec.requirement.available(observation, local) {
                names.insert(spec.definition.name.to_string());
            } else {
                unavailable
                    .push(json!({"tool":spec.definition.name,"requires":spec.requirement.label()}));
            }
        }
        if agent.role != Role::Root {
            names.extend(
                self.mcp
                    .definitions()
                    .into_iter()
                    .map(|definition| definition.name.to_string()),
            );
        }
        ToolSelection {
            names,
            environment: json!({"environment_id":agent.environment_id,"unavailable_tools":unavailable}),
        }
    }

    pub(crate) async fn prepare_tool_selection(&self, agent: &AgentMetadata) -> ToolSelection {
        // Root orchestration never connects implicitly. Explicit doctor tools can probe.
        if agent.role == Role::Root || agent.environment_id.is_none() {
            return self.selection(agent, None);
        }
        let result = tokio::time::timeout(Duration::from_secs(20), async {
            self.checked_environment(agent).await?.doctor().await
        })
        .await;
        let observation = match result {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => Err(format!("{error:#}")),
            Err(_) => Err("environment preflight timed out after 20 seconds".into()),
        };
        let mut selection = self.selection(agent, observation.as_ref().ok());
        selection.environment["preflight"] = match observation {
            Ok(value) => {
                json!({"status":"observed","identity":value["identity"],"os":value["os"],"capabilities":value["capabilities"],"methods":value["methods"]})
            }
            Err(error) => json!({"status":"unavailable","error":error}),
        };
        selection
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn observed_selection_and_dispatch_deny_unadvertised_capabilities() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let root = temporary.path();
        let bridge = root.join("capability_fixture.py");
        fs::write(
            &bridge,
            r#"import json,pathlib,platform,sys
root=pathlib.Path(sys.argv[sys.argv.index('--state')+1]);root.mkdir(exist_ok=True)
identity={'protocol':3,'installation_id':'selection-fixture','boot_id':'boot','package_sha256':'0'*64}
for line in sys.stdin:
 request=json.loads(line);method=request['method']
 if method=='ping': result={'identity':identity}
 elif method=='doctor': result={'os':platform.system().lower(),'identity':identity,'capabilities':{'resumable_file_transfer':True},'methods':{'file.put':{'ready':True}}}
 else:
  (root/'unexpected-effect').write_text(method);result={}
 print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':result}),flush=True)
"#,
        )?;
        let mut config = AppConfig {
            default_environment: Some("fixture".into()),
            ..Default::default()
        };
        config.environments.insert(
            "fixture".into(),
            crate::environments::EnvironmentConfig {
                name: "Fixture".into(),
                platform: crate::environments::Platform::current(),
                installation_id: None,
                runtime: crate::runtime::RuntimeConfig {
                    transport: crate::runtime::TransportConfig::TrustedLocal(
                        crate::runtime::TrustedLocalConfig {
                            state_dir: root.join("worker"),
                            bridge_path: bridge,
                            ..Default::default()
                        },
                    ),
                    ..Default::default()
                },
            },
        );
        let store = Store::open(root.join("state"))?;
        let coordinator = Coordinator::new(store.clone());
        let target = root.join("target.bin");
        fs::write(&target, b"trusted static fixture")?;
        let run = coordinator.create_run(
            &target,
            "selection",
            Budget::default(),
            serde_json::to_value(&config)?,
        )?;
        let child = coordinator
            .create_child(
                &run.root_id,
                crate::coordinator::ChildRequest {
                    name: "Analyst".into(),
                    role: Role::Analyst,
                    task: "Inspect".into(),
                    scope: "fixture".into(),
                    environment_id: None,
                    skills: vec![],
                    inherit_context: false,
                    wait_for_predecessors: vec![],
                    dependency_policy: Default::default(),
                    contract: Default::default(),
                },
            )?
            .agent;
        let tools = ToolService::new(config, store.clone()).await?;
        let root_agent: AgentMetadata = store.load(&run.root_id)?;
        let root_selection = tools.prepare_tool_selection(&root_agent).await;
        assert!(root_selection.names.contains("environment_doctor"));
        assert!(
            !root.join("worker").exists(),
            "root selection must stay lazy"
        );
        let selection = tools.prepare_tool_selection(&child).await;
        assert_eq!(selection.environment["preflight"]["status"], "observed");
        assert!(selection.names.contains("file_put"));
        assert!(selection.names.contains("binary_inspect"));
        assert!(!selection.names.contains("exec_command"));
        assert!(!selection.names.contains("debug_start"));
        let denied = tools
            .execute(
                &child.id,
                "forged-execution",
                "exec_command",
                json!({"command":"must not execute"}),
            )
            .await?;
        assert!(denied.is_error);
        assert!(
            denied
                .value
                .to_string()
                .contains("observed backend capability"),
            "{denied:?}"
        );
        assert!(!root.join("worker/unexpected-effect").exists());
        tools.close().await;
        Ok(())
    }
}
