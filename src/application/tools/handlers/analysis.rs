//! Analysis tool handlers.
use super::*;

pub(in crate::application::tools) fn handle_artifact_validate<'a>(
    tools: &'a ExecutionService,
    agent: &'a AgentMetadata,
    call: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        super::super::artifact_validation::start(tools, agent, call, serde_json::from_value(args)?)
            .await
    })
}

pub(in crate::application::tools) fn handle_binary_inspect<'a>(
    tools: &'a AnalysisService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    __name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        let (run, _secondary) = tools.resolve_target(agent, &args)?;
        let full = serde_json::to_value(binary::inspect(&run.target_path)?)?;
        let bytes = serde_json::to_vec(&full)?;
        let evidence = tools.store.artifact(
                    &agent.run_id,
                    &agent.id,
                    call_id,
                    "binary_inspect.full",
                    &bytes,
                    "application/json",
                    json!({"raw_tool_output":true,"tool":"binary_inspect","target_sha256":run.target_sha256}),
                )?;
        let pick = |key: &str| full.get(key).cloned().unwrap_or(Value::Null);
        let count = |key: &str| {
            full.get(key)
                .and_then(Value::as_array)
                .map_or(0, |a| a.len())
        };
        Ok(json!({
            "format": pick("format"),
            "architecture": pick("architecture"),
            "bits": pick("bits"),
            "endianness": pick("endianness"),
            "kind": pick("kind"),
            "image_base": pick("image_base"),
            "entry": pick("entry"),
            "mitigations": pick("mitigations"),
            "libraries": pick("libraries"),
            "has_debug_symbols": pick("has_debug_symbols"),
            "warnings": pick("warnings"),
            "sha256": pick("sha256"),
            "size": pick("size"),
            "imports_count": count("imports"),
            "sections_count": count("sections"),
            "symbols_count": count("symbols"),
            "full_evidence_id": evidence.id,
            "full_bytes": std::fs::metadata(&run.target_path)
                .map(|metadata| metadata.len())
                .unwrap_or(0),
        }))
    })
}

pub(in crate::application::tools) fn handle_binary_strings<'a>(
    tools: &'a AnalysisService,
    agent: &'a AgentMetadata,
    _call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        let (run, _secondary) = tools.resolve_target(agent, &args)?;
        let mut query_args = args;
        query_args
            .as_object_mut()
            .context("arguments must be an object")?
            .remove("target_ref");
        let query: StringsQuery = serde_json::from_value(query_args)?;
        Ok(serde_json::to_value(binary::strings(
            &run.target_path,
            &query,
        )?)?)
    })
}

pub(in crate::application::tools) fn handle_functions_search<'a>(
    tools: &'a AnalysisService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { tools.ghidra(agent, call_id, name, &args).await })
}

pub(in crate::application::tools) fn handle_analysis_verify<'a>(
    tools: &'a evidence::EvidenceService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        ensure!(
            agent.role == Role::Verifier,
            "independent verifier required"
        );
        let mut input: args::AnalysisVerifyArgs = serde_json::from_value(args.clone())?;
        for field in [&input.claim, &input.scope, &input.expected, &input.observed] {
            ensure!(
                !field.trim().is_empty() && field.len() <= 16384,
                "invalid verification text"
            );
        }
        ensure!(
            input.counterexamples.len() <= 32
                && input.counterexamples.iter().all(|s| s.len() <= 4096),
            "too many counterexamples"
        );
        ensure!(
            !input.evidence_ids.is_empty() && input.evidence_ids.len() <= 128,
            "verification requires bounded evidence"
        );
        tools
            .store
            .verify_evidence_links(&agent.run_id, &input.evidence_ids)?;
        tools.effect(agent, call_id, |tx| {
            analysis_verification::resolve_status_evidence(tx, agent, &mut input)?;
            let producer: AgentMetadata = tx
                .get("agent", &input.producer_id)?
                .context("claim producer missing")?;
            ensure!(
                producer.run_id == agent.run_id
                    && producer.id != agent.id
                    && producer.role != Role::Verifier,
                "invalid independent producer"
            );
            let mut own = false;
            for id in &input.evidence_ids {
                let evidence: Evidence = tx.get_record(id)?.context("evidence missing")?;
                ensure!(evidence.run_id == agent.run_id, "cross-run evidence");
                own |= evidence.agent_id == agent.id;
            }
            ensure!(own, "verification requires verifier-owned observations");
            let run: RunRecord = tx.get("run", &agent.run_id)?.context("run missing")?;
            let coverage = analysis_verification::validate(tx, agent, &run, &input)?;
            let id = stable("analysis-verification", call_id);
            let record = json!({"id":id,"run_id":agent.run_id,"verifier_id":agent.id,
                "target_sha256":run.target_sha256,"created_at":now(),"requested":args,"result":serde_json::to_value(&input)?,"coverage":coverage});
            ensure!(
                tx.get::<Value>("analysis_verification", &id)?.is_none(),
                "verification is immutable"
            );
            tx.put("analysis_verification", &id, &agent.run_id, &record)?;
            let note = SharedDocument {
                id: id.clone(),
                run_id: agent.run_id.clone(),
                agent_id: agent.id.clone(),
                kind: "note".into(),
                title: format!("Independent verification: {}", input.scope),
                body: serde_json::to_string_pretty(&record)?,
                amendments: vec![],
                created_at: now(),
                updated_at: now(),
            };
            tx.save_document(&note)?;
            tx.event(
                &agent.run_id,
                Some(&agent.id),
                "analysis.verified",
                record.clone(),
            )?;
            Ok(record)
        })
    })
}

pub(in crate::application::tools) fn handle_differential_test<'a>(
    tools: &'a ExecutionService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        tools
            .differential(agent, call_id, serde_json::from_value(args)?)
            .await
    })
}
