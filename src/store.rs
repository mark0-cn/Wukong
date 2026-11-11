pub(crate) mod desktop_queries;
mod finding_queries;
pub(crate) mod report_queries;
mod workspace;
use crate::domain::*;
use anyhow::{Context, Result, anyhow, bail};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

mod records;
pub use records::{DocumentKind, SessionKind, StoredRecord};
mod session;
pub(crate) use session::StateSession;
mod conversations;
mod leases;
pub(crate) use leases::LeaseToken;

#[derive(Clone)]
pub struct Store {
    connection: Arc<Mutex<Connection>>,
    reader: Arc<Mutex<Connection>>,
    root: Arc<PathBuf>,
    changes: tokio::sync::watch::Sender<u64>,
    fence: Option<LeaseToken>,
}

impl Store {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        crate::cleanup::create_owned_directory(root.as_ref())?;
        let root = fs::canonicalize(root.as_ref())?;
        fs::create_dir_all(root.join("artifacts"))?;
        let conn = Connection::open(root.join("state.sqlite3"))?;
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;
          CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
          INSERT OR IGNORE INTO meta VALUES('schema_version','3');
          ",
        )?;
        let version: String = conn.query_row(
            "SELECT value FROM meta WHERE key='schema_version'",
            [],
            |row| row.get(0),
        )?;
        conversations::initialize(&conn, &version)?;
        conn.execute_batch("CREATE TABLE IF NOT EXISTS entities(kind TEXT NOT NULL,id TEXT NOT NULL,run_id TEXT NOT NULL,data TEXT NOT NULL,PRIMARY KEY(kind,id));
          CREATE INDEX IF NOT EXISTS entities_run ON entities(run_id,kind);
          CREATE TABLE IF NOT EXISTS messages(id INTEGER PRIMARY KEY AUTOINCREMENT,run_id TEXT NOT NULL,sender TEXT NOT NULL,recipient TEXT NOT NULL,content TEXT NOT NULL,acknowledged INTEGER NOT NULL DEFAULT 0,dedupe_key TEXT UNIQUE,created_at TEXT NOT NULL,envelope TEXT NOT NULL);
          CREATE INDEX IF NOT EXISTS messages_inbox ON messages(recipient,acknowledged,id);
          CREATE TABLE IF NOT EXISTS events(seq INTEGER PRIMARY KEY AUTOINCREMENT,run_id TEXT NOT NULL,agent_id TEXT,kind TEXT NOT NULL,data TEXT NOT NULL,created_at TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS leases(run_id TEXT PRIMARY KEY,owner TEXT NOT NULL,expires_at INTEGER NOT NULL);")?;
        conn.execute_batch("CREATE TABLE IF NOT EXISTS lease_epochs(run_id TEXT PRIMARY KEY,epoch INTEGER NOT NULL);")?;
        // A bounded change index, shared by all processes opening this store.
        // Replacing a row assigns a new monotonic revision without retaining
        // one change record per historical update.
        conn.execute_batch("CREATE TABLE IF NOT EXISTS entity_revisions(
          revision INTEGER PRIMARY KEY AUTOINCREMENT,kind TEXT NOT NULL,id TEXT NOT NULL,
          run_id TEXT NOT NULL,UNIQUE(kind,id));
          INSERT OR IGNORE INTO entity_revisions(kind,id,run_id) SELECT kind,id,run_id FROM entities;
          CREATE TRIGGER IF NOT EXISTS entities_revision_insert AFTER INSERT ON entities BEGIN
            INSERT INTO entity_revisions(kind,id,run_id) VALUES(NEW.kind,NEW.id,NEW.run_id) ON CONFLICT(kind,id) DO UPDATE SET revision=excluded.revision,run_id=excluded.run_id;
          END;
          CREATE TRIGGER IF NOT EXISTS entities_revision_update AFTER UPDATE ON entities BEGIN
            INSERT INTO entity_revisions(kind,id,run_id) VALUES(NEW.kind,NEW.id,NEW.run_id) ON CONFLICT(kind,id) DO UPDATE SET revision=excluded.revision,run_id=excluded.run_id;
          END;
          CREATE TRIGGER IF NOT EXISTS entities_revision_delete AFTER DELETE ON entities BEGIN
            INSERT INTO entity_revisions(kind,id,run_id) VALUES(OLD.kind,OLD.id,OLD.run_id) ON CONFLICT(kind,id) DO UPDATE SET revision=excluded.revision,run_id=excluded.run_id;
          END;")?;
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS events_run_seq ON events(run_id,seq);
          CREATE INDEX IF NOT EXISTS events_agent_kind ON events(run_id,agent_id,kind,seq);
          CREATE INDEX IF NOT EXISTS revisions_run_kind ON entity_revisions(kind,run_id,revision);",
        )?;
        let reader = Connection::open_with_flags(
            root.join("state.sqlite3"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        reader.busy_timeout(std::time::Duration::from_secs(10))?;
        reader.execute_batch("PRAGMA query_only=ON;")?;
        Ok(Self {
            connection: Arc::new(Mutex::new(conn)),
            reader: Arc::new(Mutex::new(reader)),
            root: Arc::new(root),
            changes: tokio::sync::watch::channel(0).0,
            fence: None,
        })
    }
    pub(crate) fn subscribe_changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.changes.subscribe()
    }

    pub fn root(&self) -> &Path {
        self.root.as_path()
    }
    /// A consistent WAL snapshot, without taking the writer mutex or reserving
    /// a write transaction. The connection itself rejects accidental writes.
    pub fn read_transaction<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let mut reader = self
            .reader
            .lock()
            .map_err(|_| anyhow!("state reader mutex poisoned"))?;
        let tx = reader.transaction_with_behavior(rusqlite::TransactionBehavior::Deferred)?;
        let result = f(&tx)?;
        tx.commit()?;
        Ok(result)
    }
    pub fn transaction<T>(
        &self,
        f: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        let mut conn = self
            .connection
            .lock()
            .map_err(|_| anyhow!("state database mutex poisoned"))?;
        let before = conn.total_changes();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if let Some(fence) = &self.fence {
            leases::check(&tx, fence)?;
        }
        let result = f(&tx)?;
        tx.commit()?;
        if conn.total_changes() != before {
            self.changes
                .send_modify(|revision| *revision = revision.wrapping_add(1));
        }
        Ok(result)
    }
    pub fn tx_get<T: DeserializeOwned>(tx: &Connection, kind: &str, id: &str) -> Result<Option<T>> {
        let row = tx
            .query_row(
                "SELECT e.data,c.checkpoint,c.inherited_context FROM entities e
            LEFT JOIN agent_conversations c ON e.kind='agent' AND c.agent_id=e.id
            WHERE e.kind=?1 AND e.id=?2",
                params![kind, id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()?;
        row.map(|(data, checkpoint, inherited)| conversations::decode(data, checkpoint, inherited))
            .transpose()
    }
    pub(crate) fn tx_get_metadata(
        tx: &Connection,
        kind: &str,
        id: &str,
    ) -> Result<Option<AgentMetadata>> {
        anyhow::ensure!(kind == "agent", "metadata APIs accept only agent metadata");
        let data: Option<String> = tx
            .query_row(
                "SELECT data FROM entities WHERE kind=?1 AND id=?2",
                params![kind, id],
                |row| row.get(0),
            )
            .optional()?;
        data.map(|data| Ok(serde_json::from_str(&data)?))
            .transpose()
    }
    pub fn tx_put<T: Serialize>(
        tx: &Connection,
        kind: &str,
        id: &str,
        run_id: &str,
        value: &T,
    ) -> Result<()> {
        Self::tx_write(tx, kind, id, run_id, value, true)
    }
    #[cfg(test)]
    pub(crate) fn tx_put_metadata(
        tx: &Connection,
        kind: &str,
        id: &str,
        run_id: &str,
        value: &AgentMetadata,
    ) -> Result<()> {
        anyhow::ensure!(
            kind == "agent" && id == value.id && run_id == value.run_id,
            "agent metadata identity mismatch"
        );
        Self::tx_write(tx, kind, id, run_id, value, false)
    }
    fn tx_write<T: Serialize>(
        tx: &Connection,
        kind: &str,
        id: &str,
        run_id: &str,
        value: &T,
        include_history: bool,
    ) -> Result<()> {
        let mut data = serde_json::to_value(value)?;
        if kind == "agent" {
            let object = data
                .as_object_mut()
                .context("agent metadata is not an object")?;
            let checkpoint = object.remove("checkpoint");
            let inherited = object.remove("inherited_context");
            if include_history {
                conversations::write(tx, id, run_id, checkpoint, inherited)?;
            }
        }
        tx.execute("INSERT INTO entities(kind,id,run_id,data) VALUES(?1,?2,?3,?4) ON CONFLICT(kind,id) DO UPDATE SET data=excluded.data",params![kind,id,run_id,serde_json::to_string(&data)?])?;
        Ok(())
    }
    pub fn tx_list<T: DeserializeOwned>(
        tx: &Connection,
        kind: &str,
        run_id: &str,
    ) -> Result<Vec<T>> {
        let mut query = tx.prepare(
            "SELECT e.data,c.checkpoint,c.inherited_context FROM entities e
            LEFT JOIN agent_conversations c ON e.kind='agent' AND c.agent_id=e.id
            WHERE e.kind=?1 AND e.run_id=?2 ORDER BY e.rowid",
        )?;
        let rows = query.query_map(params![kind, run_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?;
        rows.map(|row| {
            let (data, checkpoint, inherited) = row?;
            conversations::decode(data, checkpoint, inherited)
        })
        .collect()
    }
    pub(crate) fn tx_list_metadata(
        tx: &Connection,
        kind: &str,
        run_id: &str,
    ) -> Result<Vec<AgentMetadata>> {
        anyhow::ensure!(kind == "agent", "metadata APIs accept only agent metadata");
        let mut query =
            tx.prepare("SELECT data FROM entities WHERE kind=?1 AND run_id=?2 ORDER BY rowid")?;
        let rows = query.query_map(params![kind, run_id], |row| row.get::<_, String>(0))?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }
    pub fn get<T: DeserializeOwned>(&self, kind: &str, id: &str) -> Result<Option<T>> {
        self.read_transaction(|tx| Self::tx_get(tx, kind, id))
    }
    pub fn agent_snapshot(&self, id: &str) -> Result<AgentSnapshot> {
        self.read_transaction(|tx| Self::tx_agent_snapshot(tx, id))?
            .with_context(|| format!("agent {id} does not exist"))
    }
    pub(crate) fn tx_agent_snapshot(tx: &Connection, id: &str) -> Result<Option<AgentSnapshot>> {
        let Some(metadata) = Self::tx_get_metadata(tx, "agent", id)? else {
            return Ok(None);
        };
        let conversation = Self::tx_agent_conversation(tx, id)?;
        Ok(Some(AgentSnapshot::from_parts(metadata, conversation)))
    }
    pub(crate) fn tx_agent_conversation(tx: &Connection, id: &str) -> Result<AgentConversation> {
        let payload: Option<(Option<String>, Option<String>)> = tx
            .query_row(
                "SELECT checkpoint,inherited_context FROM agent_conversations WHERE agent_id=?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((checkpoint, inherited)) = payload else {
            return Ok(AgentConversation::default());
        };
        Ok(AgentConversation {
            checkpoint: checkpoint
                .map(|data| serde_json::from_str(&data))
                .transpose()?,
            inherited_context: inherited
                .map(|data| serde_json::from_str(&data))
                .transpose()?,
        })
    }

    pub fn require<T: DeserializeOwned>(&self, kind: &str, id: &str) -> Result<T> {
        self.get(kind, id)?
            .ok_or_else(|| anyhow!("{kind} {id} does not exist"))
    }

    pub fn put<T: Serialize>(&self, kind: &str, id: &str, run_id: &str, value: &T) -> Result<()> {
        self.transaction(|tx| Self::tx_put(tx, kind, id, run_id, value))
    }
    pub fn list<T: DeserializeOwned>(&self, kind: &str, run_id: &str) -> Result<Vec<T>> {
        self.read_transaction(|tx| Self::tx_list(tx, kind, run_id))
    }

    pub fn runs(&self) -> Result<Vec<RunRecord>> {
        self.read_transaction(|tx| {
            let mut q =
                tx.prepare("SELECT data FROM entities WHERE kind='run' ORDER BY rowid DESC")?;
            let rows = q
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows.into_iter()
                .map(|s| Ok(serde_json::from_str(&s)?))
                .collect()
        })
    }
    pub fn tx_event(
        tx: &Connection,
        run_id: &str,
        agent_id: Option<&str>,
        kind: &str,
        data: Value,
    ) -> Result<i64> {
        tx.execute(
            "INSERT INTO events(run_id,agent_id,kind,data,created_at) VALUES(?1,?2,?3,?4,?5)",
            params![
                run_id,
                agent_id,
                kind,
                serde_json::to_string(&data)?,
                now().to_rfc3339()
            ],
        )?;
        Ok(tx.last_insert_rowid())
    }
    pub fn event(
        &self,
        run_id: &str,
        agent_id: Option<&str>,
        kind: &str,
        data: Value,
    ) -> Result<i64> {
        self.transaction(|tx| Self::tx_event(tx, run_id, agent_id, kind, data))
    }
    pub fn events(&self, run_id: &str, after: i64, limit: usize) -> Result<Vec<Event>> {
        self.read_transaction(|tx|{
            let mut q=tx.prepare("SELECT seq,agent_id,kind,data,created_at FROM events WHERE run_id=?1 AND seq>?2 ORDER BY seq LIMIT ?3")?;
            let rows=q.query_map(params![run_id,after,limit.min(10000) as i64],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;
            rows.into_iter().map(|(seq,agent_id,kind,data,at)|Ok(Event{seq,run_id:run_id.to_owned(),agent_id,kind,data:serde_json::from_str(&data)?,created_at:at.parse()?})).collect()
        })
    }
    /// Latest events in ascending sequence order, without truncating to the oldest window.
    pub fn recent_events(&self, run_id: &str, limit: usize) -> Result<Vec<Event>> {
        self.read_transaction(|tx| Self::tx_recent_events(tx, run_id, limit))
    }
    pub(crate) fn tx_recent_events(
        tx: &Connection,
        run_id: &str,
        limit: usize,
    ) -> Result<Vec<Event>> {
        let mut q=tx.prepare("SELECT seq,agent_id,kind,data,created_at FROM events WHERE run_id=?1 ORDER BY seq DESC LIMIT ?2")?;
        let rows = q
            .query_map(params![run_id, limit.min(10000) as i64], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows.into_iter()
            .rev()
            .map(|(seq, agent_id, kind, data, at)| {
                Ok(Event {
                    seq,
                    run_id: run_id.to_owned(),
                    agent_id,
                    kind,
                    data: serde_json::from_str(&data)?,
                    created_at: at.parse()?,
                })
            })
            .collect()
    }
    pub fn tx_message(
        tx: &Connection,
        run_id: &str,
        sender: &str,
        recipient: &str,
        content: &str,
        key: Option<&str>,
    ) -> Result<bool> {
        let target: AgentMetadata =
            Self::tx_get_metadata(tx, "agent", recipient)?.context("recipient missing")?;
        let envelope = MessageEnvelope {
            options: MessageOptions {
                kind: MessageKind::Result,
                recipient_generation: Some(target.generation),
                ..Default::default()
            },
            task_id: recipient.into(),
            sender_generation: Self::tx_get_metadata(tx, "agent", sender)?
                .map(|agent| agent.generation),
            ..Default::default()
        };
        let (_, inserted) =
            Self::tx_enqueue_message(tx, run_id, sender, recipient, content, key, &envelope)?;
        Ok(inserted)
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn tx_enqueue_message(
        tx: &Connection,
        run_id: &str,
        sender: &str,
        recipient: &str,
        content: &str,
        key: Option<&str>,
        envelope: &MessageEnvelope,
    ) -> Result<(i64, bool)> {
        let inserted = tx.execute("INSERT OR IGNORE INTO messages(run_id,sender,recipient,content,dedupe_key,created_at,envelope,acknowledged) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![run_id,sender,recipient,content,key,now().to_rfc3339(),serde_json::to_string(envelope)?,envelope.delivery == MessageDelivery::Stale])? > 0;
        let mid = if inserted {
            tx.last_insert_rowid()
        } else {
            tx.query_row("SELECT id FROM messages WHERE dedupe_key=?1", [key], |r| {
                r.get(0)
            })?
        };
        Ok((mid, inserted))
    }
    pub fn tx_messages(tx: &Connection, run_id: &str) -> Result<Vec<Message>> {
        let mut query = tx.prepare("SELECT json_object('id',id,'run_id',run_id,'sender',sender,'recipient',recipient,'content',content,'acknowledged',json(CASE WHEN acknowledged=1 THEN 'true' ELSE 'false' END),'created_at',created_at,'envelope',json(envelope)) FROM messages WHERE run_id=?1 ORDER BY id")?;
        let rows = query
            .query_map([run_id], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|value| Ok(serde_json::from_str(&value)?))
            .collect()
    }
    pub fn inbox(&self, recipient: &str) -> Result<Vec<Message>> {
        self.read_transaction(|tx| {
            let agent: AgentMetadata =
                Self::tx_get_metadata(tx, "agent", recipient)?.context("recipient missing")?;
            Ok(Self::tx_messages(tx, &agent.run_id)?
                .into_iter()
                .filter(|message| {
                    message.recipient == recipient
                        && !message.acknowledged
                        && message.envelope.delivery != MessageDelivery::Stale
                })
                .collect())
        })
    }
    pub fn acquire_lease(&self, run_id: &str, owner: &str, seconds: i64) -> Result<bool> {
        Ok(self.claim_lease(run_id, owner, seconds)?.is_some())
    }
    pub fn release_lease(&self, run_id: &str, owner: &str) -> Result<()> {
        self.transaction(|tx| {
            tx.execute(
                "DELETE FROM leases WHERE run_id=?1 AND owner=?2",
                params![run_id, owner],
            )?;
            Ok(())
        })
    }
    // The provenance tuple and payload deliberately stay explicit at call sites.
    #[allow(clippy::too_many_arguments)]
    pub fn artifact(
        &self,
        run_id: &str,
        agent_id: &str,
        call_id: &str,
        source: &str,
        bytes: &[u8],
        media_type: &str,
        metadata: Value,
    ) -> Result<Evidence> {
        let hash = hex::encode(Sha256::digest(bytes));
        let relative = format!("artifacts/{}/{}", &hash[..2], hash);
        let path = self.root.join(&relative);
        fs::create_dir_all(path.parent().context("artifact parent")?)?;
        if !path.exists() {
            let parent = path.parent().context("artifact parent")?;
            let mut staged = tempfile::NamedTempFile::new_in(parent)?;
            staged.write_all(bytes)?;
            staged.as_file().sync_all()?;
            if let Err(error) = staged.persist_noclobber(&path)
                && error.error.kind() != std::io::ErrorKind::AlreadyExists
            {
                return Err(error.error).context("publish evidence artifact");
            }
            #[cfg(unix)]
            fs::File::open(parent)?.sync_all()?;
        }
        // Verify both a newly published artifact and one won by another writer.
        let existing = fs::read(&path)?;
        if hex::encode(Sha256::digest(&existing)) != hash {
            bail!("artifact integrity failure at {}", path.display());
        }
        let evidence = Evidence {
            id: id("ev"),
            run_id: run_id.to_owned(),
            agent_id: agent_id.to_owned(),
            tool_call_id: call_id.to_owned(),
            sha256: hash,
            relative_path: relative,
            size: bytes.len() as u64,
            media_type: media_type.to_owned(),
            source: source.to_owned(),
            metadata,
            created_at: now(),
        };
        self.put("evidence", &evidence.id, run_id, &evidence)?;
        Ok(evidence)
    }
    pub fn read_evidence(&self, run_id: &str, evidence_id: &str) -> Result<Vec<u8>> {
        let record: Evidence = self.require("evidence", evidence_id)?;
        if record.run_id != run_id {
            bail!("evidence belongs to another run");
        }
        let path = fs::canonicalize(self.root.join(&record.relative_path))?;
        if !path.starts_with(self.root.as_path()) {
            bail!("artifact escapes state directory");
        }
        let bytes = fs::read(path)?;
        if hex::encode(Sha256::digest(&bytes)) != record.sha256 {
            bail!("artifact digest mismatch");
        }
        Ok(bytes)
    }

    /// Stream-verify the whole artifact digest and return only the requested
    /// range, keeping memory bounded for large evidence files.
    pub fn read_evidence_range(
        &self,
        run_id: &str,
        evidence_id: &str,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>> {
        let record: Evidence = self.require("evidence", evidence_id)?;
        if record.run_id != run_id {
            bail!("evidence belongs to another run");
        }
        let path = fs::canonicalize(self.root.join(&record.relative_path))?;
        if !path.starts_with(self.root.as_path()) {
            bail!("artifact escapes state directory");
        }
        let mut file = fs::File::open(path)?;
        if file.metadata()?.len() != record.size {
            bail!("artifact size changed");
        }
        let mut digest = Sha256::new();
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
        if hex::encode(digest.finalize()) != record.sha256 {
            bail!("artifact digest mismatch");
        }
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = vec![0_u8; length];
        let count = file.read(&mut bytes)?;
        bytes.truncate(count);
        Ok(bytes)
    }
    pub fn verify_evidence_links(&self, run_id: &str, ids: &[String]) -> Result<()> {
        for evidence in ids {
            self.read_evidence(run_id, evidence)?;
        }
        Ok(())
    }
    pub fn checkpoint_and_ack(
        &self,
        agent_id: &str,
        checkpoint: Value,
        message_ids: &[i64],
    ) -> Result<()> {
        self.checkpoint_and_ack_generation(agent_id, checkpoint, message_ids, None)
    }
    pub(crate) fn checkpoint_and_ack_generation(
        &self,
        agent_id: &str,
        checkpoint: Value,
        message_ids: &[i64],
        generation: Option<u64>,
    ) -> Result<()> {
        self.transaction(|tx| {
            let agent: AgentMetadata =
                Self::tx_get_metadata(tx, "agent", agent_id)?.context("agent missing")?;
            if generation.is_some_and(|expected| expected != agent.generation) {
                return Ok(());
            }
            conversations::write(tx,agent_id,&agent.run_id,Some(checkpoint),None)?;
            for mid in message_ids {
                tx.execute(
                    "UPDATE messages SET acknowledged=1,envelope=json_set(envelope,'$.delivery',CASE WHEN json_extract(envelope,'$.delivery')='replied' THEN 'replied' ELSE 'included' END) WHERE id=?1 AND recipient=?2 AND COALESCE(json_extract(envelope,'$.delivery'),'queued')!='stale'",
                    params![mid, agent_id],
                )?;
            }
            Ok(())
        })
    }
    pub fn finish_call(&self, record: &ToolCallRecord) -> Result<()> {
        self.transaction(|tx|{Self::tx_put(tx,"call",&record.id,&record.run_id,record)?;Self::tx_event(tx,&record.run_id,Some(&record.agent_id),"tool.finished",json!({"call_id":record.id,"name":record.name,"state":record.state,"evidence_id":record.evidence_id}))?;Ok(())})
    }
}
struct UtcTimestamp;
impl UtcTimestamp {
    fn get() -> i64 {
        chrono::Utc::now().timestamp()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_state_schema_is_rejected_without_rewriting_records() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let conn = Connection::open(directory.path().join("state.sqlite3"))?;
        conn.execute_batch(
            "CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
            INSERT INTO meta VALUES('schema_version','2');
            CREATE TABLE legacy_history(data TEXT); INSERT INTO legacy_history VALUES('retained');",
        )?;
        let error = match Store::open(directory.path()) {
            Ok(_) => panic!("old schema accepted"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("expected 3"));
        let history: String =
            conn.query_row("SELECT data FROM legacy_history", [], |row| row.get(0))?;
        assert_eq!(history, "retained");
        Ok(())
    }

    #[test]
    fn metadata_writes_and_duplicate_checkpoints_do_not_rewrite_conversations() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let target = directory.path().join("sample");
        fs::write(&target, b"fixture")?;
        let store = Store::open(directory.path().join("state"))?;
        let run = crate::coordinator::Coordinator::new(store.clone()).create_run(
            &target,
            "fixture",
            Budget::default(),
            json!({}),
        )?;
        let checkpoint = json!({"messages":["x".repeat(500_000)]});
        store.checkpoint_and_ack(&run.root_id, checkpoint.clone(), &[])?;
        let revision = || {
            store.read_transaction(|tx| {
                Ok(tx.query_row(
                    "SELECT revision FROM agent_conversations WHERE agent_id=?1",
                    [&run.root_id],
                    |row| row.get::<_, i64>(0),
                )?)
            })
        };
        let before = revision()?;
        for _ in 0..20 {
            store.transaction(|tx| {
                let mut agent: AgentMetadata =
                    Store::tx_get_metadata(tx, "agent", &run.root_id)?.context("missing")?;
                let metadata = serde_json::to_value(&agent)?;
                assert!(metadata.get("checkpoint").is_none());
                assert!(metadata.get("inherited_context").is_none());
                agent.total_tokens += 10;
                Store::tx_put_metadata(tx, "agent", &agent.id, &run.id, &agent)
            })?;
        }
        store.checkpoint_and_ack(&run.root_id, checkpoint.clone(), &[])?;
        store.checkpoint_and_ack_generation(
            &run.root_id,
            json!({"wrong generation":true}),
            &[],
            Some(42),
        )?;
        assert_eq!(revision()?, before);
        let full = store.agent_snapshot(&run.root_id)?;
        assert_eq!(full.checkpoint, Some(checkpoint));
        assert_eq!(full.total_tokens, 200);
        let wire = serde_json::to_value(&full)?;
        assert_eq!(wire["id"], run.root_id);
        assert!(wire.get("metadata").is_none());
        let restored: AgentSnapshot = serde_json::from_value(wire)?;
        assert_eq!(restored.checkpoint, full.checkpoint);
        assert_eq!(restored.metadata.total_tokens, full.metadata.total_tokens);
        Ok(())
    }

    #[test]
    fn readers_see_committed_state_while_another_writer_is_active() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let store = Store::open(directory.path())?;
        store.put("fixture", "item", "run", &json!({"value":"committed"}))?;
        let mut external = Connection::open(directory.path().join("state.sqlite3"))?;
        let tx = external.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute(
            "UPDATE entities SET data=?1 WHERE kind='fixture' AND id='item'",
            [json!({"value":"pending"}).to_string()],
        )?;
        let before: Value = store.require("fixture", "item")?;
        assert_eq!(before["value"], "committed");
        tx.commit()?;
        let after: Value = store.require("fixture", "item")?;
        assert_eq!(after["value"], "pending");
        assert!(
            store
                .read_transaction(|tx| {
                    tx.execute("DELETE FROM entities", [])?;
                    Ok(())
                })
                .is_err()
        );
        assert!(store.get::<Value>("fixture", "item")?.is_some());
        Ok(())
    }

    #[test]
    fn concurrent_artifact_publication_never_overwrites_corrupt_content() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let store = Store::open(directory.path())?;
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        let writers: Vec<_> = (0..4)
            .map(|index| {
                let store = store.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    store.artifact(
                        "run",
                        "agent",
                        &format!("call-{index}"),
                        "test",
                        b"same evidence",
                        "text/plain",
                        json!({}),
                    )
                })
            })
            .collect();
        let evidence: Vec<_> = writers
            .into_iter()
            .map(|writer| writer.join().unwrap())
            .collect::<Result<_>>()?;
        assert!(
            evidence
                .iter()
                .all(|entry| entry.relative_path == evidence[0].relative_path)
        );
        let path = store.root().join(&evidence[0].relative_path);
        assert_eq!(fs::read(&path)?, b"same evidence");
        assert_eq!(fs::read_dir(path.parent().unwrap())?.count(), 1);
        fs::write(&path, b"corrupted")?;
        assert!(
            store
                .artifact(
                    "run",
                    "agent",
                    "next",
                    "test",
                    b"same evidence",
                    "text/plain",
                    json!({})
                )
                .is_err()
        );
        assert_eq!(fs::read(path)?, b"corrupted");
        Ok(())
    }

    #[test]
    fn range_reads_verify_the_streamed_digest_and_stay_bounded() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path().join("state"))?;
        let bytes = b"0123456789abcdef".repeat(4096); // 64 KiB
        let evidence = store.artifact(
            "run-1",
            "agent-1",
            "call-1",
            "test",
            &bytes,
            "application/octet-stream",
            serde_json::json!({}),
        )?;
        let range = store.read_evidence_range("run-1", &evidence.id, 1000, 4000)?;
        assert_eq!(&range, &bytes[1000..5000]);
        // Cross-run references stay rejected.
        assert!(
            store
                .read_evidence_range("other-run", &evidence.id, 0, 10)
                .is_err()
        );
        // Tampering (here: a size change) must be detected.
        let path = store.root().join(&evidence.relative_path);
        fs::write(&path, b"tampered")?;
        assert!(
            store
                .read_evidence_range("run-1", &evidence.id, 0, 10)
                .is_err()
        );
        Ok(())
    }
}
