//! One SQLite snapshot produces the web workspace, without leaking model configuration/checkpoints.
use super::*;
use crate::{
    desktop::{presentation, workspace::*},
    queries::{AgentSummary, RunSummary},
};
use std::collections::BTreeSet;
mod narration;
mod tools;
pub(crate) fn read(
    store: &Store,
    epoch: u64,
    request: &WorkspaceRequest,
) -> Result<WorkspaceResponse> {
    store.read_transaction(|tx| {
        let mut query=tx.prepare("SELECT json_remove(data,'$.config') FROM entities WHERE kind='run' ORDER BY rowid DESC")?;
        let runs=query.query_map([],|row|row.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?.into_iter().map(|data|Ok(serde_json::from_str::<RunSummary>(&data)?)).collect::<Result<Vec<_>>>()?;
        let selected=request.run.clone().or_else(||runs.first().map(|run|run.id.clone()));
        if let Some(id)=&selected {anyhow::ensure!(runs.iter().any(|run|&run.id==id),"分析任务不存在");}
        let mut mail_query=tx.prepare("SELECT json_object('id',id,'run_id',run_id,'sender',sender,'recipient',recipient,'content',content,'acknowledged',json(CASE WHEN acknowledged=1 THEN 'true' ELSE 'false' END),'created_at',created_at,'envelope',json(envelope)) FROM messages WHERE run_id=?1 AND sender='operator' AND (?2 IS NULL OR recipient=?2) ORDER BY id DESC LIMIT 500")?;
        let mail=mail_query.query_map(params![selected,request.agent],|row|row.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let revision:i64=tx.query_row("SELECT COALESCE(MAX(revision),0) FROM entity_revisions",[],|row|row.get(0))?;
        let latest:i64=tx.query_row("SELECT COALESCE(MAX(seq),0) FROM events WHERE run_id=?1",[&selected],|row|row.get(0))?;
        let controller_live:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM leases WHERE run_id=?1 AND expires_at>?2)",params![selected,chrono::Utc::now().timestamp()],|row|row.get(0))?;
        let signature=hex::encode(Sha256::digest(serde_json::to_vec(&(epoch,revision,latest,&selected,&request.agent,request.event_limit,&mail,controller_live))?));
        if request.known_revision.as_deref()==Some(&signature) {return Ok(WorkspaceResponse {revision:signature,data:None});}
        let mut data=WorkspaceData {selected:selected.clone(),runs,agents:vec![],findings:vec![],notes:vec![],transcript:vec![],active_tools:vec![],has_earlier:false};
        if let Some(run)=&selected {
            data.agents=Store::tx_list_metadata(tx,"agent",run)?.into_iter().map(AgentSummary::from).collect();
            if let Some(agent)=&request.agent {anyhow::ensure!(data.agents.iter().any(|value|&value.id==agent),"Agent 不属于当前分析任务");}
            data.findings=Store::tx_list(tx,"finding",run)?;data.notes=Store::tx_list(tx,"note",run)?;
            let limit=request.event_limit.unwrap_or(500).clamp(50,10_000);
            let mut query=tx.prepare("SELECT seq,agent_id,kind,data,created_at FROM events WHERE run_id=?1 AND (?2 IS NULL OR agent_id=?2) ORDER BY seq DESC LIMIT ?3")?;
            let mut events=query.query_map(params![run,request.agent,limit+1],|row|Ok((row.get::<_,i64>(0)?,row.get::<_,Option<String>>(1)?,row.get::<_,String>(2)?,row.get::<_,String>(3)?,row.get::<_,String>(4)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
            data.has_earlier=events.len()>limit;events.truncate(limit);events.reverse();
            let events=events.into_iter().map(|(seq,agent_id,kind,raw,created)|Ok(Event{seq,run_id:run.clone(),agent_id,kind,data:serde_json::from_str(&raw)?,created_at:created.parse()?})).collect::<Result<Vec<_>>>()?;
            let target_name=data.runs.iter().find(|item| &item.id==run).and_then(|item| item.target_path.file_name()).map(|name|name.to_string_lossy().into_owned()).unwrap_or_default();
            let tools=tools::read(tx,data.runs.iter().find(|item| &item.id==run).expect("selected run exists"),request.agent.as_deref(),&events,&data.agents)?;
            data.active_tools=tools.active;
            let mail=mail.into_iter().map(|raw|Ok(serde_json::from_str::<Message>(&raw)?)).collect::<Result<Vec<_>>>()?;
            let replies=narration::read(tx,run,&events)?;
            data.transcript=presentation::transcript(&events,&tools.calls,&data.agents,&mail,&presentation::TranscriptState {target_name:&target_name,jobs:&tools.jobs,replies:&replies,controller_live});
        }
        Ok(WorkspaceResponse{revision:signature,data:Some(data)})
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn returned_call_tracks_job_outcome_without_duplicate_rows() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path())?;
        let run = crate::coordinator::Coordinator::new(store.clone()).create_run(
            &std::env::current_exe()?,
            "fixture",
            Budget::default(),
            json!({}),
        )?;
        let agent: AgentMetadata = store.load(&run.root_id)?;
        let call = ToolCallRecord {
            id: "replay-call".into(),
            run_id: run.id.clone(),
            agent_id: agent.id.clone(),
            name: "crash_replay".into(),
            arguments: json!({"target":"parser.elf"}),
            state: CallState::Completed,
            side_effecting: true,
            result: Some(json!({"job_id":"replay-job"})),
            evidence_id: None,
            created_at: now(),
            updated_at: now(),
        };
        store.save_record(&call)?;
        store.event(
            &run.id,
            Some(&agent.id),
            "tool.finished",
            json!({"call_id":call.id}),
        )?;
        let mut job: JobRecord = serde_json::from_value(json!({
            "id":"replay-job", "run_id":run.id, "agent_id":agent.id, "worker_id":"fixture",
            "owner_generation":agent.generation, "call_id":call.id, "worker_request_id":"request",
            "tool_name":call.name, "finding_id":null, "parameters":{}, "target_sha256":run.target_sha256,
            "status":{"job_id":"replay-job", "state":"running", "started_at":1700000000},
            "status_evidence_id":null, "imported_artifacts":null, "notification_message_id":null,
            "created_at":now(), "updated_at":now()
        }))?;
        store.save_record(&job)?;
        let request = WorkspaceRequest {
            run: Some(run.id.clone()),
            ..Default::default()
        };
        let first = read(&store, 1, &request)?;
        let data = first.data.unwrap();
        assert_eq!(data.active_tools.len(), 1);
        assert_eq!(data.active_tools[0].state, "running");
        assert_eq!(data.transcript.len(), 1);
        // A terminal job updates the same row even though the request already completed.
        job.status.state = JobState::Completed;
        job.status.result = Some(serde_json::from_value(
            json!({"passed":false,"attempts":2,"successes":1}),
        )?);
        job.status
            .extra
            .insert("finished_at".into(), json!(1700000004.5));
        store.save_record(&job)?;
        let data = read(
            &store,
            1,
            &WorkspaceRequest {
                known_revision: Some(first.revision),
                ..request.clone()
            },
        )?
        .data
        .unwrap();
        assert!(data.active_tools.is_empty());
        assert_eq!(data.transcript.len(), 1);
        let presentation::ChatPart::ToolCall { args, is_error, .. } =
            &data.transcript[0].content[0]
        else {
            panic!("tool missing")
        };
        assert!(*is_error);
        assert_eq!(args["display"]["duration_ms"], 4500);
        assert_eq!(args["display"]["job"]["successes"], 1);
        // Later status observations must not inflate the worker's duration.
        job.updated_at = now() + chrono::Duration::hours(1);
        store.save_record(&job)?;
        assert_eq!(
            serde_json::to_value(read(&store, 1, &request)?.data)?["transcript"][0]["content"][0]["args"]
                ["display"]["duration_ms"],
            4500
        );
        // A stale generation or mismatched target cannot become current toolbar work.
        job.status.state = JobState::Running;
        job.owner_generation += 1;
        store.save_record(&job)?;
        assert!(
            read(&store, 1, &request)?
                .data
                .unwrap()
                .active_tools
                .is_empty()
        );
        job.owner_generation = agent.generation;
        job.target_sha256 = "other-target".into();
        store.save_record(&job)?;
        assert!(
            read(&store, 1, &request)?
                .data
                .unwrap()
                .active_tools
                .is_empty()
        );
        job.target_sha256 = run.target_sha256.clone();
        job.status.state = JobState::Completed;
        for (result, expected) in [
            (json!({"exit_code":1}), "failed"),
            (json!({"exit_code":139,"passed":true}), "completed"),
        ] {
            job.status.result = Some(serde_json::from_value(result)?);
            store.save_record(&job)?;
            let data = serde_json::to_value(read(&store, 1, &request)?.data)?;
            assert_eq!(
                data["transcript"][0]["content"][0]["args"]["display"]["state"],
                expected
            );
        }
        Ok(())
    }
    #[test]
    fn snapshot_is_scoped_incremental_and_excludes_private_configuration() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path())?;
        let run = crate::coordinator::Coordinator::new(store.clone()).create_run(
            &std::env::current_exe()?,
            "fixture",
            Budget::default(),
            json!({"secret":"excluded"}),
        )?;
        let mut request = WorkspaceRequest {
            run: Some(run.id.clone()),
            ..Default::default()
        };
        let initial = read(&store, 1, &request)?;
        assert!(!serde_json::to_string(&initial)?.contains("secret"));
        request.known_revision = Some(initial.revision);
        assert!(read(&store, 1, &request)?.data.is_none());
        assert!(read(&store, 2, &request)?.data.is_some());
        request.agent = Some("other".into());
        assert!(read(&store, 1, &request).is_err());
        Ok(())
    }
}
