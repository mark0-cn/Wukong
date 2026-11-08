use super::*;
use crate::queries::ReportSnapshot;
pub(crate) fn snapshot(store: &Store, run_id: &str) -> Result<ReportSnapshot> {
    store.read_transaction(|tx| {
        let run = Store::tx_get(tx, "run", run_id)?.context("run does not exist")?;
        let mut query = tx.prepare(
            "SELECT seq,agent_id,kind,data,created_at FROM events WHERE run_id=?1 ORDER BY seq",
        )?;
        let rows = query
            .query_map([run_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let events = rows
            .into_iter()
            .map(|(seq, agent_id, kind, data, at)| {
                Ok(Event {
                    seq,
                    agent_id,
                    kind,
                    data: serde_json::from_str(&data)?,
                    created_at: at.parse()?,
                    run_id: run_id.into(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let messages = Store::tx_messages(tx, run_id)?;
        let mut documents = Vec::new();
        for kind in ["document", "threat_model", "note", "todo"] {
            documents.extend(Store::tx_list::<SharedDocument>(tx, kind, run_id)?);
        }
        Ok(ReportSnapshot {
            run,
            agents: Store::tx_list(tx, "agent", run_id)?,
            findings: Store::tx_list(tx, "finding", run_id)?,
            verifications: Store::tx_list(tx, "verification", run_id)?,
            coverage: Store::tx_list(tx, "coverage", run_id)?,
            evidence: Store::tx_list(tx, "evidence", run_id)?,
            calls: Store::tx_list(tx, "call", run_id)?,
            documents,
            jobs: Store::tx_list(tx, "job", run_id)?,
            worker_files: Store::tx_list(tx, "worker_file", run_id)?,
            debug_sessions: Store::tx_list(tx, "debug_session", run_id)?,
            events,
            messages,
            task_results: Store::tx_list(tx, "task_result", run_id)?,
        })
    })
}
