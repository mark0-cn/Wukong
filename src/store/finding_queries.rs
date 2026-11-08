//! Scoped finding/evidence read models for the desktop inspector.
use super::*;
use crate::queries::{EvidencePreview, FindingDetails, FindingQueries};
impl FindingQueries<'_> {
    pub fn details(&self, run: &str, id: &str) -> Result<FindingDetails> {
        self.store.read_transaction(|tx| {
            let finding: Finding = Store::tx_get(tx, "finding", id)?.context("分析发现不存在")?;
            anyhow::ensure!(finding.run_id == run, "finding belongs to another run");
            let verification: Option<Verification> = finding
                .verification_id
                .as_deref()
                .map(|id| Store::tx_get(tx, "verification", id))
                .transpose()?
                .flatten();
            if let Some(verification) = &verification {
                anyhow::ensure!(
                    verification.run_id == run && verification.finding_id == id,
                    "verification belongs to another finding"
                );
            }
            let mut ids: std::collections::BTreeSet<_> =
                finding.evidence_ids.iter().cloned().collect();
            if let Some(verification) = &verification {
                ids.extend(verification.evidence_ids.iter().cloned());
            }
            let mut evidence = Vec::new();
            let mut missing_evidence = Vec::new();
            let mut calls = std::collections::BTreeMap::new();
            for id in ids {
                let Some(record): Option<Evidence> = Store::tx_get(tx, "evidence", &id)? else {
                    missing_evidence.push(id);
                    continue;
                };
                anyhow::ensure!(record.run_id == run, "evidence belongs to another run");
                if let Some(call) =
                    Store::tx_get::<ToolCallRecord>(tx, "call", &record.tool_call_id)?
                {
                    anyhow::ensure!(
                        call.run_id == run,
                        "source tool call belongs to another run"
                    );
                    calls.insert(call.id.clone(), call);
                }
                evidence.push(record);
            }
            Ok(FindingDetails {
                finding,
                verification,
                evidence,
                calls: calls.into_values().collect(),
                missing_evidence,
            })
        })
    }
    pub fn preview(&self, run: &str, id: &str, offset: u64) -> Result<EvidencePreview> {
        let evidence: Evidence = self.store.require("evidence", id)?;
        anyhow::ensure!(evidence.run_id == run, "evidence belongs to another run");
        anyhow::ensure!(
            offset <= evidence.size,
            "preview offset exceeds evidence size"
        );
        // Integrity and path checks are shared with tool evidence reads. Memory stays bounded.
        let bytes = self.store.read_evidence_range(run, id, offset, 16 * 1024)?;
        Ok(EvidencePreview {
            evidence,
            offset,
            bytes,
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inspector_scopes_and_bounds_artifact_reads() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path())?;
        let record = store.artifact(
            "run",
            "agent",
            "call",
            "fixture",
            &vec![b'x'; 32 * 1024],
            "text/plain",
            serde_json::json!({}),
        )?;
        let finding: Finding = serde_json::from_value(serde_json::json!({
            "id":"finding", "run_id":"run", "agent_id":"agent", "title":"fixture", "description":"description",
            "stage":"suspected", "evidence_ids":[record.id,"missing"], "created_at":now(), "updated_at":now()
        }))?;
        store.save_record(&finding)?;
        let query = FindingQueries::new(&store);
        let details = query.details("run", "finding")?;
        assert_eq!(details.evidence.len(), 1);
        assert_eq!(details.missing_evidence, vec!["missing"]);
        assert!(query.details("other", "finding").is_err());
        assert!(query.preview("other", &record.id, 0).is_err());
        let first = query.preview("run", &record.id, 0)?;
        assert_eq!(first.bytes.len(), 16 * 1024);
        let second = query.preview("run", &record.id, first.bytes.len() as u64)?;
        assert_eq!(second.bytes.len(), 16 * 1024);
        let path = store.root().join(&record.relative_path);
        std::fs::write(path, vec![b'y'; 32 * 1024])?;
        assert!(query.preview("run", &record.id, 0).is_err());
        Ok(())
    }
}
