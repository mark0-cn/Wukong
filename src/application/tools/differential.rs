//! Compare two real LaunchSpec jobs, retaining ordinary job ownership and evidence.
use super::*;

fn compare(rule: &args::ComparisonRule, left: &[u8], right: &[u8]) -> Option<bool> {
    match rule {
        args::ComparisonRule::ExactOutput => Some(left == right),
        args::ComparisonRule::ExitStatus => Some(true),
        args::ComparisonRule::StdoutJson => Some(
            serde_json::from_slice::<Value>(left).ok()?
                == serde_json::from_slice::<Value>(right).ok()?,
        ),
    }
}

impl ExecutionService {
    async fn comparison_stream(
        &self,
        agent: &AgentMetadata,
        job: &str,
        stream: &str,
    ) -> Result<Option<(String, Vec<u8>)>> {
        let job = self.jobs.known_job(agent, job, true)?;
        let Some(items) = job
            .imported_artifacts
            .as_ref()
            .and_then(|v| v["artifacts"].as_array())
        else {
            return Ok(None);
        };
        for item in items {
            let path = item["artifact"]["path"].as_str().unwrap_or_default();
            if path.ends_with(&format!("/{stream}")) || path.ends_with(&format!("/output.{stream}"))
            {
                let id = text(item, "evidence_id")?;
                let evidence: Evidence = require_in_run(&self.store, id, &agent.run_id)?;
                if evidence.size >= 65536 {
                    return Ok(None);
                }
                return Ok(Some((
                    id.into(),
                    self.store.read_evidence(&agent.run_id, id)?,
                )));
            }
        }
        Ok(None)
    }

    pub(super) async fn differential(
        &self,
        agent: &AgentMetadata,
        call: &str,
        input: args::DifferentialTestArgs,
    ) -> Result<Value> {
        let inputs = input
            .input_evidence_ids
            .iter()
            .map(|id| {
                let evidence: Evidence = require_in_run(&self.store, id, &agent.run_id)?;
                ensure!(evidence.size <= 65536, "differential input exceeds 64 KiB");
                Ok(evidence)
            })
            .collect::<Result<Vec<_>>>()?;
        let mut identities = Vec::new();
        for path in [&input.target_launch_spec, &input.reference_launch_spec] {
            self.files.known_file(agent, path, false)?;
            let file = self
                .files
                .file_get(
                    agent,
                    &format!("{call}:manifest:{path}"),
                    &json!({"path":path,"max_bytes":65536}),
                )
                .await?;
            let id = text(&file, "content_evidence_id")?;
            let manifest: Value =
                serde_json::from_slice(&self.store.read_evidence(&agent.run_id, id)?)?;
            ensure!(
                (1..=10).contains(&number(&manifest, "timeout_secs", 0)),
                "differential LaunchSpecs must each record a 1..10 second timeout"
            );
            ensure!(
                manifest["input_mode"] == "stdin_pipe" || manifest["input_mode"] == "argv_file",
                "differential requires stdin_pipe or argv_file"
            );
            identities.push(json!({"launch_spec":path,"evidence_id":id,"manifest":manifest}));
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
        let mut rows = Vec::new();
        let mut budget_exhausted = false;
        'cases: for (index, id) in input.input_evidence_ids.iter().enumerate() {
            let evidence = &inputs[index];
            let staged = self
                .files
                .file_put(
                    agent,
                    &format!("{call}:input:{index}"),
                    &json!({"path":format!("diff/{}.bin",evidence.sha256),"evidence_id":id}),
                )
                .await?;
            let mut jobs = Vec::new();
            for (side, launch) in [&input.target_launch_spec, &input.reference_launch_spec]
                .iter()
                .enumerate()
            {
                if tokio::time::Instant::now() >= deadline {
                    budget_exhausted = true;
                    rows.push(json!({"input_evidence_id":id,"job_ids":jobs,"verdict":"inconclusive","reason":"comparison budget expired"}));
                    break 'cases;
                }
                let started=self.runtime_tool(agent,&format!("{call}:case:{index}:{side}"),"job_start",json!({"launch_spec":launch,"input":staged["file"]["path"],"output_limit":65536})).await?;
                let job = text(&started, "job_id")?.to_owned();
                jobs.push(job.clone());
                // Sequential starts also work on the single-resource Windows persona.
                loop {
                    self.jobs
                        .observe(agent, &format!("{call}:observe:{job}"), &job)
                        .await?;
                    if self.jobs.known_job(agent, &job, true)?.status.settled()
                        || tokio::time::Instant::now() >= deadline
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }
            let mut observations = Vec::new();
            let mut outputs = Vec::new();
            let mut complete = true;
            let mut exits = Vec::new();
            for id in &jobs {
                let job = self.jobs.known_job(agent, id, true)?;
                let exit = job.status.result.as_ref().and_then(|r| r.exit_code);
                let ready = job.status.settled()
                    && job.status.state == JobState::Completed
                    && exit.is_some_and(|code| input.expected_exit_codes.contains(&code));
                complete &= ready;
                if ready {
                    self.jobs
                        .import_job_artifacts(agent, &format!("{call}:artifacts:{id}"), id)
                        .await?;
                }
                let stdout = self.comparison_stream(agent, id, "stdout").await?;
                let stderr = self.comparison_stream(agent, id, "stderr").await?;
                complete &= match input.comparison {
                    args::ComparisonRule::ExitStatus => true,
                    args::ComparisonRule::StdoutJson => stdout.is_some(),
                    args::ComparisonRule::ExactOutput => stdout.is_some() && stderr.is_some(),
                };
                observations.push(json!({"job_id":id,"exit_code":exit,"state":job.status.state,"status_evidence_id":job.status_evidence_id,"stdout_evidence_id":stdout.as_ref().map(|s|&s.0),"stderr_evidence_id":stderr.as_ref().map(|s|&s.0)}));
                exits.push(exit);
                outputs.push((
                    stdout.map(|s| s.1).unwrap_or_default(),
                    stderr.map(|s| s.1).unwrap_or_default(),
                ));
            }
            let equal = compare(&input.comparison, &outputs[0].0, &outputs[1].0);
            let verdict = if !complete || equal.is_none() {
                "inconclusive"
            } else if equal == Some(true)
                && exits[0] == exits[1]
                && (!matches!(input.comparison, args::ComparisonRule::ExactOutput)
                    || outputs[0].1 == outputs[1].1)
            {
                "match"
            } else {
                "mismatch"
            };
            rows.push(json!({"input_evidence_id":id,"input_sha256":evidence.sha256,"observations":observations,"verdict":verdict}));
            if tokio::time::Instant::now() >= deadline {
                budget_exhausted = true;
                break;
            }
        }
        let result = json!({"comparison":input.comparison,"launches":identities,"cases":rows,"requested_cases":input.input_evidence_ids.len(),"complete":!budget_exhausted && rows.len()==input.input_evidence_ids.len(),"matches":rows.iter().filter(|r|r["verdict"]=="match").count(),"mismatches":rows.iter().filter(|r|r["verdict"]=="mismatch").count(),"inconclusive":rows.iter().filter(|r|r["verdict"]=="inconclusive").count()});
        let evidence = self.store.artifact(
            &agent.run_id,
            &agent.id,
            call,
            "differential.results",
            &serde_json::to_vec(&result)?,
            "application/json",
            json!({"raw_tool_output":true}),
        )?;
        Ok(
            json!({"result_evidence_id":evidence.id,"cases":result["cases"],"complete":result["complete"],"matches":result["matches"],"mismatches":result["mismatches"],"inconclusive":result["inconclusive"]}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn comparison_rules_are_explicit_and_invalid_json_is_inconclusive() {
        assert_eq!(
            compare(&args::ComparisonRule::ExactOutput, b"1\n", b"1"),
            Some(false)
        );
        assert_eq!(
            compare(
                &args::ComparisonRule::StdoutJson,
                b"{\"a\":1,\"b\":2}",
                b"{\"b\":2,\"a\":1}"
            ),
            Some(true)
        );
        assert_eq!(
            compare(&args::ComparisonRule::StdoutJson, b"invalid", b"invalid"),
            None
        );
    }
}
