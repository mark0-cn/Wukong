//! Optional conversation payloads live outside frequently updated agent metadata.
use super::*;

pub(super) fn initialize(conn: &Connection, version: &str) -> Result<()> {
    if version != "3" {
        bail!("unsupported state schema {version}; expected 3; select a fresh state directory");
    }
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS agent_conversations(
        agent_id TEXT PRIMARY KEY,run_id TEXT NOT NULL,checkpoint TEXT,inherited_context TEXT,
        revision INTEGER NOT NULL DEFAULT 1);",
    )?;
    Ok(())
}

pub(super) fn decode<T: DeserializeOwned>(
    data: String,
    checkpoint: Option<String>,
    inherited: Option<String>,
) -> Result<T> {
    if checkpoint.is_none() && inherited.is_none() {
        return Ok(serde_json::from_str(&data)?);
    }
    let mut data: Value = serde_json::from_str(&data)?;
    let object = data
        .as_object_mut()
        .context("agent metadata is not an object")?;
    if let Some(checkpoint) = checkpoint {
        object.insert("checkpoint".into(), serde_json::from_str(&checkpoint)?);
    }
    if let Some(inherited) = inherited {
        object.insert(
            "inherited_context".into(),
            serde_json::from_str(&inherited)?,
        );
    }
    Ok(serde_json::from_value(data)?)
}

pub(super) fn write(
    tx: &Connection,
    id: &str,
    run_id: &str,
    checkpoint: Option<Value>,
    inherited: Option<Value>,
) -> Result<()> {
    if checkpoint.is_none() && inherited.is_none() {
        return Ok(());
    }
    let has_checkpoint = checkpoint.is_some();
    let has_inherited = inherited.is_some();
    let encode = |value: Option<Value>| {
        value
            .filter(|value| !value.is_null())
            .map(|value| serde_json::to_string(&value))
            .transpose()
    };
    let checkpoint = encode(checkpoint)?;
    let inherited = encode(inherited)?;
    tx.execute("INSERT INTO agent_conversations(agent_id,run_id,checkpoint,inherited_context) VALUES(?1,?2,?3,?4)
        ON CONFLICT(agent_id) DO UPDATE SET
          checkpoint=CASE WHEN ?5 THEN excluded.checkpoint ELSE agent_conversations.checkpoint END,
          inherited_context=CASE WHEN ?6 THEN excluded.inherited_context ELSE agent_conversations.inherited_context END,
          revision=agent_conversations.revision+1
        WHERE (?5 AND agent_conversations.checkpoint IS NOT excluded.checkpoint)
           OR (?6 AND agent_conversations.inherited_context IS NOT excluded.inherited_context)",
        params![id,run_id,checkpoint,inherited,has_checkpoint,has_inherited])?;
    Ok(())
}
