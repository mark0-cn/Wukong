use crate::domain::*;
use anyhow::{Context, Result, anyhow, bail};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[derive(Clone)]
pub struct Store {
    connection: Arc<Mutex<Connection>>,
    root: Arc<PathBuf>,
}

impl Store {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        fs::create_dir_all(root.as_ref())?;
        let root = fs::canonicalize(root.as_ref())?;
        fs::create_dir_all(root.join("artifacts"))?;
        let conn = Connection::open(root.join("state.sqlite3"))?;
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;
          CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
          INSERT OR IGNORE INTO meta VALUES('schema_version','2');
          CREATE TABLE IF NOT EXISTS entities(kind TEXT NOT NULL,id TEXT NOT NULL,run_id TEXT NOT NULL,data TEXT NOT NULL,PRIMARY KEY(kind,id));
          CREATE INDEX IF NOT EXISTS entities_run ON entities(run_id,kind);
          CREATE TABLE IF NOT EXISTS messages(id INTEGER PRIMARY KEY AUTOINCREMENT,run_id TEXT NOT NULL,sender TEXT NOT NULL,recipient TEXT NOT NULL,content TEXT NOT NULL,acknowledged INTEGER NOT NULL DEFAULT 0,dedupe_key TEXT UNIQUE,created_at TEXT NOT NULL,envelope TEXT NOT NULL);
          CREATE INDEX IF NOT EXISTS messages_inbox ON messages(recipient,acknowledged,id);
          CREATE TABLE IF NOT EXISTS events(seq INTEGER PRIMARY KEY AUTOINCREMENT,run_id TEXT NOT NULL,agent_id TEXT,kind TEXT NOT NULL,data TEXT NOT NULL,created_at TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS leases(run_id TEXT PRIMARY KEY,owner TEXT NOT NULL,expires_at INTEGER NOT NULL);")?;
        let version: String = conn.query_row(
            "SELECT value FROM meta WHERE key='schema_version'",
            [],
            |r| r.get(0),
        )?;
        if version != "2" {
            bail!("unsupported state schema {version}; use a fresh state directory");
        }
        Ok(Self {
            connection: Arc::new(Mutex::new(conn)),
            root: Arc::new(root),
        })
    }
    pub fn root(&self) -> &Path {
        self.root.as_path()
    }
    pub fn transaction<T>(
        &self,
        f: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        let mut conn = self
            .connection
            .lock()
            .map_err(|_| anyhow!("state database mutex poisoned"))?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let result = f(&tx)?;
        tx.commit()?;
        Ok(result)
    }
    pub fn tx_get<T: DeserializeOwned>(tx: &Connection, kind: &str, id: &str) -> Result<Option<T>> {
        let data: Option<String> = tx
            .query_row(
                "SELECT data FROM entities WHERE kind=?1 AND id=?2",
                params![kind, id],
                |r| r.get(0),
            )
            .optional()?;
        data.map(|s| serde_json::from_str(&s).context("decode durable record"))
            .transpose()
    }
    pub fn tx_put<T: Serialize>(
        tx: &Connection,
        kind: &str,
        id: &str,
        run_id: &str,
        value: &T,
    ) -> Result<()> {
        tx.execute("INSERT INTO entities(kind,id,run_id,data) VALUES(?1,?2,?3,?4) ON CONFLICT(kind,id) DO UPDATE SET data=excluded.data",params![kind,id,run_id,serde_json::to_string(value)?])?;
        Ok(())
    }
    pub fn tx_list<T: DeserializeOwned>(
        tx: &Connection,
        kind: &str,
        run_id: &str,
    ) -> Result<Vec<T>> {
        let mut q =
            tx.prepare("SELECT data FROM entities WHERE kind=?1 AND run_id=?2 ORDER BY rowid")?;
        let strings = q
            .query_map(params![kind, run_id], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        strings
            .into_iter()
            .map(|s| Ok(serde_json::from_str(&s)?))
            .collect()
    }
    pub fn get<T: DeserializeOwned>(&self, kind: &str, id: &str) -> Result<Option<T>> {
        self.transaction(|tx| Self::tx_get(tx, kind, id))
    }
    pub fn require<T: DeserializeOwned>(&self, kind: &str, id: &str) -> Result<T> {
        self.get(kind, id)?
            .ok_or_else(|| anyhow!("{kind} {id} does not exist"))
    }
    pub fn put<T: Serialize>(&self, kind: &str, id: &str, run_id: &str, value: &T) -> Result<()> {
        self.transaction(|tx| Self::tx_put(tx, kind, id, run_id, value))
    }
    pub fn list<T: DeserializeOwned>(&self, kind: &str, run_id: &str) -> Result<Vec<T>> {
        self.transaction(|tx| Self::tx_list(tx, kind, run_id))
    }
    pub fn runs(&self) -> Result<Vec<RunRecord>> {
        self.transaction(|tx| {
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
        self.transaction(|tx|{
            let mut q=tx.prepare("SELECT seq,agent_id,kind,data,created_at FROM events WHERE run_id=?1 AND seq>?2 ORDER BY seq LIMIT ?3")?;
            let rows=q.query_map(params![run_id,after,limit.min(10000) as i64],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;
            rows.into_iter().map(|(seq,agent_id,kind,data,at)|Ok(Event{seq,run_id:run_id.to_owned(),agent_id,kind,data:serde_json::from_str(&data)?,created_at:at.parse()?})).collect()
        })
    }
    /// Latest events in ascending sequence order, without truncating to the oldest window.
    pub fn recent_events(&self, run_id: &str, limit: usize) -> Result<Vec<Event>> {
        self.transaction(|tx|{
            let mut q=tx.prepare("SELECT seq,agent_id,kind,data,created_at FROM events WHERE run_id=?1 ORDER BY seq DESC LIMIT ?2")?;
            let rows=q.query_map(params![run_id,limit.min(10000) as i64],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;
            rows.into_iter().rev().map(|(seq,agent_id,kind,data,at)|Ok(Event{seq,run_id:run_id.to_owned(),agent_id,kind,data:serde_json::from_str(&data)?,created_at:at.parse()?})).collect()
        })
    }
    pub fn tx_message(
        tx: &Connection,
        run_id: &str,
        sender: &str,
        recipient: &str,
        content: &str,
        key: Option<&str>,
    ) -> Result<bool> {
        let target: AgentRecord =
            Self::tx_get(tx, "agent", recipient)?.context("recipient missing")?;
        let envelope = MessageEnvelope {
            options: MessageOptions {
                kind: MessageKind::Result,
                recipient_generation: Some(target.generation),
                ..Default::default()
            },
            task_id: recipient.into(),
            sender_generation: Self::tx_get::<AgentRecord>(tx, "agent", sender)?
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
        self.transaction(|tx| {
            let agent: AgentRecord =
                Self::tx_get(tx, "agent", recipient)?.context("recipient missing")?;
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
        self.transaction(|tx|{
            let timestamp=UtcTimestamp::get();
            Ok(tx.execute("INSERT INTO leases(run_id,owner,expires_at) VALUES(?1,?2,?3) ON CONFLICT(run_id) DO UPDATE SET owner=excluded.owner,expires_at=excluded.expires_at WHERE leases.owner=excluded.owner OR leases.expires_at<?4",params![run_id,owner,timestamp+seconds,timestamp])?>0)
        })
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
        if path.exists() {
            let existing = fs::read(&path)?;
            if hex::encode(Sha256::digest(&existing)) != hash {
                bail!("artifact integrity failure at {}", path.display());
            }
        } else {
            let temp = path.with_extension(format!("{}.tmp", id("write")));
            fs::write(&temp, bytes)?;
            // Another writer can have committed identical bytes meanwhile.
            if let Err(e) = fs::rename(&temp, &path) {
                if path.exists() {
                    fs::remove_file(&temp)?;
                } else {
                    return Err(e.into());
                }
            }
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
            let mut agent: AgentRecord =
                Self::tx_get(tx, "agent", agent_id)?.context("agent missing")?;
            if generation.is_some_and(|expected| expected != agent.generation) {
                return Ok(());
            }
            agent.checkpoint = Some(checkpoint);
            agent.updated_at = now();
            Self::tx_put(tx, "agent", agent_id, &agent.run_id, &agent)?;
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
