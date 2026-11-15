//! Read a single bounded workspace snapshot without desktop presentation or model configuration.
use super::*;
use crate::queries::{
    models::{AgentSummary, RunSummary},
    workspace::*,
};
use std::collections::BTreeSet;
mod narration;
mod tools;
pub(crate) fn read(
    store: &Store,
    epoch: u64,
    request: &WorkspaceRequest,
) -> Result<WorkspaceSnapshotReply> {
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
        if request.known_revision.as_deref()==Some(&signature) {return Ok(WorkspaceSnapshotReply {revision:signature,data:None});}
        let mut data=WorkspaceSnapshot {selected:selected.clone(),runs,..Default::default()};
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
            let tools=tools::read(tx,data.runs.iter().find(|item| &item.id==run).expect("selected run exists"),request.agent.as_deref(),&events)?;
            let mail=mail.into_iter().map(|raw|Ok(serde_json::from_str::<Message>(&raw)?)).collect::<Result<Vec<_>>>()?;
            let replies=narration::read(tx,run,&events)?;
            data.events=events; data.calls=tools.calls; data.jobs=tools.jobs; data.operator_messages=mail; data.replies=replies; data.target_name=target_name; data.controller_live=controller_live;
        }
        Ok(WorkspaceSnapshotReply{revision:signature,data:Some(data)})
    })
}
