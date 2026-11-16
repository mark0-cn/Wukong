//! Execution tool handlers.
use super::*;

pub(in crate::application::tools) fn handle_file_revise<'a>(
    tools: &'a ExecutionService,
    agent: &'a AgentMetadata,
    call: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        tools
            .files
            .revise(agent, call, serde_json::from_value(args)?)
            .await
    })
}

pub(in crate::application::tools) fn handle_artifact_stage<'a>(
    tools: &'a ExecutionService,
    agent: &'a AgentMetadata,
    call: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { tools.files.stage_artifact(agent, call, &args).await })
}

pub(in crate::application::tools) fn handle_environment_list<'a>(
    tools: &'a environment_service::EnvironmentService,
    _agent: &'a AgentMetadata,
    _call: &'a str,
    _name: &'a str,
    _args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { Ok(tools.registry.list()) })
}

pub(in crate::application::tools) fn handle_environment_doctor<'a>(
    tools: &'a environment_service::EnvironmentService,
    agent: &'a AgentMetadata,
    _call: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        let id = text(&args, "environment_id")?;
        ensure!(
            agent.role == Role::Root || agent.environment_id.as_deref() == Some(id),
            "an agent may probe only its assigned environment"
        );
        let run: RunRecord = tools.store.load(&agent.run_id)?;
        let config = tools.registry.config(id)?;
        ensure!(
            run.config["environments"][id] == serde_json::to_value(config)?,
            "environment is outside this assessment or its configuration changed"
        );
        tools.registry.doctor(id).await
    })
}

pub(in crate::application::tools) fn handle_emulate_run<'a>(
    tools: &'a ExecutionService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        tools
            .files
            .known_file(agent, text(&args, "target")?, true)?;
        tools
            .gateway
            .worker_call(
                agent,
                call_id,
                Request::EmulateRun(serde_json::from_value(args)?),
            )
            .await
    })
}

pub(in crate::application::tools) fn handle_libc_identify<'a>(
    tools: &'a ExecutionService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        if let Some(target) = args["target"].as_str() {
            tools.files.known_file(agent, target, true)?;
        }
        for path in args["candidates"].as_array().into_iter().flatten() {
            tools.files.known_file(
                agent,
                path.as_str().context("candidate path missing")?,
                false,
            )?;
        }
        tools
            .gateway
            .worker_call(
                agent,
                call_id,
                Request::LibcIdentify(serde_json::from_value(args)?),
            )
            .await
    })
}

pub(in crate::application::tools) fn handle_seccomp_analyze<'a>(
    tools: &'a ExecutionService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        let file = tools
            .files
            .known_file(agent, text(&args, "filter")?, false)?;
        let raw = tools
            .gateway
            .worker_call(
                agent,
                call_id,
                Request::SeccompAnalyze(serde_json::from_value(args)?),
            )
            .await?;
        ensure!(
            raw["filter_sha256"] == file.sha256,
            "seccomp filter changed from the registered artifact"
        );
        Ok(raw)
    })
}

pub(in crate::application::tools) fn handle_runtime_doctor<'a>(
    tools: &'a environment_service::EnvironmentService,
    agent: &'a AgentMetadata,
    _call_id: &'a str,
    _name: &'a str,
    _args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        if agent.environment_id.is_some() {
            tools.checked(agent).await?.doctor().await
        } else {
            Ok(
                json!({"enabled":false,"capabilities":{},"reason":"task has no assigned environment"}),
            )
        }
    })
}

pub(in crate::application::tools) fn handle_stage_target<'a>(
    tools: &'a ExecutionService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    _name: &'a str,
    _args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { tools.files.stage_target(agent, call_id).await })
}

pub(in crate::application::tools) fn handle_file_put<'a>(
    tools: &'a ExecutionService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { tools.files.file_put(agent, call_id, &args).await })
}

pub(in crate::application::tools) fn handle_file_get<'a>(
    tools: &'a ExecutionService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { tools.files.file_get(agent, call_id, &args).await })
}

pub(in crate::application::tools) fn handle_job_start<'a>(
    tools: &'a ExecutionService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { tools.runtime_tool(agent, call_id, name, args).await })
}

pub(in crate::application::tools) fn handle_runtime_profile<'a>(
    tools: &'a ExecutionService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        for key in ["target", "libc", "loader"] {
            if let Some(path) = args[key].as_str() {
                tools.files.known_file(agent, path, key == "target")?;
            }
        }
        let mut profile_args = args.clone();
        for field in ["session_id", "interactive_session_id", "address"] {
            profile_args
                .as_object_mut()
                .context("profile arguments must be an object")?
                .remove(field);
        }
        let mut raw = tools
            .gateway
            .worker_call(
                agent,
                call_id,
                (if name == "runtime_prepare" {
                    Method::RuntimePrepare
                } else {
                    Method::RuntimeProfile
                })
                .decode(profile_args)?,
            )
            .await?;
        if let Some(files) = raw["artifacts"].as_array() {
            for file in files {
                tools.files.register_file(agent, file, None)?;
            }
        }
        ensure!(
            args["session_id"].is_null() || args["interactive_session_id"].is_null(),
            "choose one session kind"
        );
        if let Some(session_id) = args["session_id"]
            .as_str()
            .or(args["interactive_session_id"].as_str())
        {
            let debug = args["session_id"].is_string();
            let mut observation = tools
                .sessions
                .session_tool(
                    agent,
                    &format!("{call_id}:profile"),
                    if debug {
                        "debug_profile"
                    } else {
                        "interactive_profile"
                    },
                    json!({"session_id":session_id}),
                )
                .await?;
            if !debug {
                observation = json!({"response":observation});
            }
            let process = &observation["response"]["process"];
            let mut matches = serde_json::Map::new();
            for key in ["target", "libc", "loader"] {
                if let Some(hash) = raw["files"][key]["sha256"].as_str() {
                    let matched = process["modules"].as_array().is_some_and(|items| {
                        items.iter().any(|m| m["sha256"].as_str() == Some(hash))
                    });
                    matches.insert(
                        key.into(),
                        json!(if matched {
                            "mapped_hash_confirmed"
                        } else {
                            "not_confirmed"
                        }),
                    );
                }
            }
            raw["mapped_match"] = json!(
                !matches.is_empty()
                    && matches
                        .values()
                        .all(|value| value == "mapped_hash_confirmed")
            );
            raw["mapped_files"] = Value::Object(matches);
            raw["status"] = json!(if process["status"] == "observed" {
                "process_observed"
            } else {
                "process_observation_unavailable"
            });
            raw["observation"] = observation;
        }
        Ok(raw)
    })
}

pub(in crate::application::tools) fn handle_crash_snapshot<'a>(
    tools: &'a ExecutionService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        if name == "cyclic_offset" {
            tools
                .files
                .known_file(agent, text(&args, "pattern")?, false)?;
            return tools
                .gateway
                .worker_call(
                    agent,
                    call_id,
                    Request::CyclicOffset(serde_json::from_value(args)?),
                )
                .await;
        }
        ensure!(
            args["session_id"].is_string() != args["core"].is_string(),
            "choose exactly one snapshot source"
        );
        if args["session_id"].is_string() {
            tools
                .sessions
                .session_tool(agent, call_id, "debug_snapshot", args)
                .await
        } else {
            tools.files.known_file(agent, text(&args, "core")?, false)?;
            tools
                .gateway
                .worker_call(
                    agent,
                    call_id,
                    Request::CoreSnapshot(serde_json::from_value(args)?),
                )
                .await
        }
    })
}

pub(in crate::application::tools) fn handle_mitigation_profile<'a>(
    tools: &'a ExecutionService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        let runtime =
            handle_runtime_profile(tools, agent, call_id, "runtime_profile", args.clone()).await?;
        let run = target(&tools.store, agent)?;
        let binary = binary::inspect(&run.target_path)?;
        let process = &runtime["observation"]["response"]["process"];
        let mapping = args["address"].as_u64().and_then(|address| {
            process["maps"]
                .as_array()?
                .iter()
                .find(|m| {
                    m["start"].as_u64().is_some_and(|start| start <= address)
                        && m["end"].as_u64().is_some_and(|end| address < end)
                })
                .cloned()
        });
        Ok(json!({
            "static_declarations":{"target_sha256":binary.sha256,"format":binary.format,
                "mitigations":binary.mitigations,"elf_properties":runtime["files"]["target"]["elf_properties"]},
            "environment_support":runtime["environment_support"],
            "runtime_observation":runtime["observation"],
            "mapped_files":runtime["mapped_files"],
            "selected_address":args["address"], "selected_address_mapping":mapping,
            "limits":["Static declarations and CPU/kernel support do not prove process enablement.",
                "Thread observations are time-scoped; unknown and absent fields are not disabled.",
                "Seccomp mode does not reveal allowed syscalls. No filter policy was extracted.",
                "A selected mapping describes its current permissions, not the entire GOT or all future mappings."]
        }))
    })
}
