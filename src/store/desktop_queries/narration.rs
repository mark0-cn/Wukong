//! Bounded public reply snapshots for the selected event window.
use super::*;

pub(super) fn read(tx: &Connection, run: &str, events: &[Event]) -> Result<Vec<PublicReply>> {
    let ids: BTreeSet<_> = events
        .iter()
        .filter_map(|event| event.data["reply_id"].as_str())
        .collect();
    let mut query = tx.prepare("SELECT data FROM entities WHERE kind='public_reply' AND run_id=?1 AND id IN (SELECT value FROM json_each(?2)) ORDER BY rowid")?;
    query
        .query_map(params![run, serde_json::to_string(&ids)?], |row| {
            row.get::<_, String>(0)
        })?
        .map(|raw| Ok(serde_json::from_str(&raw?)?))
        .collect()
}
