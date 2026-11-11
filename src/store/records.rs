//! Record-to-storage bindings. Business callers select Rust types, never table keys.
use super::*;

pub trait StoredRecord: Serialize + DeserializeOwned {
    const KIND: &'static str;
    const HISTORY: bool = true;
    fn record_id(&self) -> &str;
    fn record_run(&self) -> &str;
}
macro_rules! record {
    ($record:ty,$kind:literal) => {
        impl StoredRecord for $record {
            const KIND: &'static str = $kind;
            fn record_id(&self) -> &str {
                &self.id
            }
            fn record_run(&self) -> &str {
                &self.run_id
            }
        }
    };
}
record!(AgentSnapshot, "agent");
record!(JobRecord, "job");
record!(WorkerFileRecord, "worker_file");
record!(EnvironmentBinding, "environment_binding");
record!(ToolCallRecord, "call");
record!(Finding, "finding");
record!(Verification, "verification");
record!(Evidence, "evidence");
record!(Coverage, "coverage");
record!(Thought, "thought");
record!(PublicReply, "public_reply");
impl StoredRecord for RunRecord {
    const KIND: &'static str = "run";
    fn record_id(&self) -> &str {
        &self.id
    }
    fn record_run(&self) -> &str {
        &self.id
    }
}
impl StoredRecord for AgentMetadata {
    const KIND: &'static str = "agent";
    const HISTORY: bool = false;
    fn record_id(&self) -> &str {
        &self.id
    }
    fn record_run(&self) -> &str {
        &self.run_id
    }
}
impl StateSession<'_> {
    pub(crate) fn get_record<T: StoredRecord>(&self, id: &str) -> Result<Option<T>> {
        if T::HISTORY {
            return Store::tx_get(self.connection, T::KIND, id);
        }
        let data: Option<String> = self
            .connection
            .query_row(
                "SELECT data FROM entities WHERE kind=?1 AND id=?2",
                params![T::KIND, id],
                |row| row.get(0),
            )
            .optional()?;
        data.map(|value| Ok(serde_json::from_str(&value)?))
            .transpose()
    }
    pub(crate) fn records<T: StoredRecord>(&self, run: &str) -> Result<Vec<T>> {
        if T::HISTORY {
            return Store::tx_list(self.connection, T::KIND, run);
        }
        let mut query = self
            .connection
            .prepare("SELECT data FROM entities WHERE kind=?1 AND run_id=?2 ORDER BY rowid")?;
        let rows = query
            .query_map(params![T::KIND, run], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|value| Ok(serde_json::from_str(&value)?))
            .collect()
    }
    pub(crate) fn save_record<T: StoredRecord>(&self, record: &T) -> Result<()> {
        Store::tx_write(
            self.connection,
            T::KIND,
            record.record_id(),
            record.record_run(),
            record,
            T::HISTORY,
        )
    }
}
impl Store {
    pub fn get_record<T: StoredRecord>(&self, id: &str) -> Result<Option<T>> {
        self.read(|tx| tx.get_record(id))
    }
    pub fn load<T: StoredRecord>(&self, id: &str) -> Result<T> {
        self.get_record(id)?
            .with_context(|| format!("{} {id} does not exist", T::KIND))
    }
    pub fn records<T: StoredRecord>(&self, run: &str) -> Result<Vec<T>> {
        self.read(|tx| tx.records(run))
    }
    pub fn save_record<T: StoredRecord>(&self, record: &T) -> Result<()> {
        self.write(|tx| tx.save_record(record))
    }
}

#[derive(Clone, Copy)]
pub enum SessionKind {
    Debugger,
    Interactive,
}
impl SessionKind {
    fn key(self) -> &'static str {
        match self {
            Self::Debugger => "debug_session",
            Self::Interactive => "interact_session",
        }
    }
}
#[derive(Clone, Copy)]
pub enum DocumentKind {
    Document,
    ThreatModel,
    Note,
    Todo,
}
impl DocumentKind {
    fn key(self) -> &'static str {
        match self {
            Self::Document => "document",
            Self::ThreatModel => "threat_model",
            Self::Note => "note",
            Self::Todo => "todo",
        }
    }
}
impl TryFrom<&str> for DocumentKind {
    type Error = anyhow::Error;
    fn try_from(kind: &str) -> Result<Self> {
        Ok(match kind {
            "document" => Self::Document,
            "threat_model" => Self::ThreatModel,
            "note" => Self::Note,
            "todo" => Self::Todo,
            _ => anyhow::bail!("invalid document kind"),
        })
    }
}
impl StateSession<'_> {
    pub(crate) fn document(&self, kind: DocumentKind, id: &str) -> Result<Option<SharedDocument>> {
        Store::tx_get(self.connection, kind.key(), id)
    }
    pub(crate) fn documents(&self, kind: DocumentKind, run: &str) -> Result<Vec<SharedDocument>> {
        Store::tx_list(self.connection, kind.key(), run)
    }
    pub(crate) fn save_document(&self, document: &SharedDocument) -> Result<()> {
        Store::tx_put(
            self.connection,
            DocumentKind::try_from(document.kind.as_str())?.key(),
            &document.id,
            &document.run_id,
            document,
        )
    }
}
impl Store {
    pub fn session(&self, kind: SessionKind, id: &str) -> Result<SessionRecord> {
        self.require(kind.key(), id)
    }
    pub fn save_session(&self, kind: SessionKind, session: &SessionRecord) -> Result<()> {
        self.put(kind.key(), &session.id, &session.run_id, session)
    }
    pub fn document(&self, kind: DocumentKind, id: &str) -> Result<SharedDocument> {
        self.read(|tx| tx.document(kind, id))?
            .context("document missing")
    }
    pub fn documents(&self, kind: DocumentKind, run: &str) -> Result<Vec<SharedDocument>> {
        self.read(|tx| tx.documents(kind, run))
    }
}

impl StateSession<'_> {
    pub(crate) fn archive_result(&self, agent: &AgentMetadata, result: &TaskResult) -> Result<()> {
        Store::tx_put(
            self.connection,
            "task_result",
            &format!("{}:{}", agent.id, agent.generation),
            &agent.run_id,
            &json!({"task_id":agent.id,"generation":agent.generation,"result":result}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn typed_metadata_updates_preserve_the_complete_conversation() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let target = dir.path().join("sample");
        fs::write(&target, b"fixture")?;
        let store = Store::open(dir.path().join("state"))?;
        let run = crate::coordinator::Coordinator::new(store.clone()).create_run(
            &target,
            "fixture",
            Budget::default(),
            json!({}),
        )?;
        let history = json!({"messages":[{"role":"assistant","text":"durable"}]});
        store.checkpoint_and_ack(&run.root_id, history.clone(), &[])?;
        let mut metadata: AgentMetadata = store.load(&run.root_id)?;
        metadata.total_tokens = 42;
        store.save_record(&metadata)?;
        let snapshot: AgentSnapshot = store.load(&run.root_id)?;
        assert_eq!(snapshot.total_tokens, 42);
        assert_eq!(snapshot.checkpoint, Some(history));
        Ok(())
    }
}
