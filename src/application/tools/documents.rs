//! Narrow controller-owned documents operations.
use super::*;

pub(super) struct DocumentService {
    pub(super) store: Store,
    pub(super) output_limit: usize,
}
impl DocumentService {
    pub(super) fn effect(
        &self,
        agent: &AgentMetadata,
        call: &str,
        action: impl FnOnce(&crate::storage::StateSession<'_>) -> Result<Value>,
    ) -> Result<Value> {
        DurableEffects::new(&self.store, self.output_limit).commit(agent, call, action)
    }
    pub(super) fn save_document(
        &self,
        agent: &AgentMetadata,
        call_id: &str,
        name: &str,
        args: &Value,
    ) -> Result<Value> {
        let kind = match name {
            "threat_model_save" => "threat_model",
            "note_save" => "note",
            _ => "todo",
        };
        self.effect(agent, call_id, |tx| {
            let document = SharedDocument {
                id: stable("doc", call_id),
                run_id: agent.run_id.clone(),
                agent_id: agent.id.clone(),
                kind: kind.into(),
                title: text(args, "title")?.into(),
                body: args["body"].as_str().unwrap_or_default().into(),
                amendments: if kind == "todo" {
                    vec![json!({"status":"pending","agent_id":agent.id,"created_at":now()})]
                } else {
                    vec![]
                },
                created_at: now(),
                updated_at: now(),
            };
            tx.save_document(&document)?;
            Ok(serde_json::to_value(document)?)
        })
    }

    pub(super) fn amend_document(
        &self,
        agent: &AgentMetadata,
        call_id: &str,
        name: &str,
        args: &Value,
    ) -> Result<Value> {
        let kind = if name == "threat_model_amend" {
            "threat_model"
        } else {
            "todo"
        };
        self.effect(agent, call_id, |tx| {
            let mut document: SharedDocument = tx
                .get(kind, text(args, "id")?)?
                .context("document missing")?;
            ensure!(
                document.run_id == agent.run_id,
                "document belongs to another run"
            );
            let mut amendment = json!({"agent_id":agent.id,"created_at":now(),"call_id":call_id});
            if name == "threat_model_amend" {
                amendment["body"] = json!(text(args, "amendment")?);
            } else {
                ensure!(
                    args.get("body").is_some() || args.get("status").is_some(),
                    "todo_update requires a body or status"
                );
                if let Some(body) = args["body"].as_str() {
                    document.body = body.into();
                    amendment["body"] = json!(body);
                }
                if let Some(status) = args["status"].as_str() {
                    amendment["status"] = json!(status);
                }
            }
            document.amendments.push(amendment);
            document.updated_at = now();
            tx.save_document(&document)?;
            Ok(serde_json::to_value(document)?)
        })
    }
}
