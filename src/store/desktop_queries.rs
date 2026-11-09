//! One SQLite snapshot produces the web workspace, without leaking model configuration/checkpoints.
use super::*;
use crate::{
    desktop::{presentation, workspace::*},
    queries::{AgentSummary, RunSummary},
};
use std::collections::BTreeSet;
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
        let signature=hex::encode(Sha256::digest(serde_json::to_vec(&(epoch,revision,latest,&selected,&request.agent,request.event_limit,&mail))?));
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
            let ids:BTreeSet<_>=events.iter().filter_map(|event|event.data["call_id"].as_str()).collect();
            let mut calls=Vec::new();for id in ids {if let Some(call)=Store::tx_get::<ToolCallRecord>(tx,"call",id)? && call.run_id==*run {calls.push(call);}}
            let mut active_query=tx.prepare("SELECT data FROM entities WHERE kind='call' AND run_id=?1 AND json_extract(data,'$.state') IN ('prepared','running','uncertain') AND (?2 IS NULL OR json_extract(data,'$.agent_id')=?2) ORDER BY rowid")?;
            for raw in active_query.query_map(params![run,request.agent],|row|row.get::<_,String>(0))? {let call:ToolCallRecord=serde_json::from_str(&raw?)?;let agent=data.agents.iter().find(|agent|agent.id==call.agent_id);let display=presentation::tool_display(&call,agent);if matches!(display.state.as_str(),"queued"|"running"|"uncertain") {data.active_tools.push(display);}}
            let mail=mail.into_iter().map(|raw|Ok(serde_json::from_str::<Message>(&raw)?)).collect::<Result<Vec<_>>>()?;
            data.transcript=presentation::transcript(&events,&calls,&data.agents,&mail);
        }
        Ok(WorkspaceResponse{revision:signature,data:Some(data)})
    })
}
#[cfg(test)]
mod tests {
    use super::*;
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
