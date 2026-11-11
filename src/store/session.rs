//! Transaction-scoped persistence API. Callers never receive a SQL connection.
use super::*;

pub(crate) struct StateSession<'a> {
    pub(super) connection: &'a Connection,
}
impl Store {
    pub(crate) fn write<T>(
        &self,
        action: impl FnOnce(&StateSession<'_>) -> Result<T>,
    ) -> Result<T> {
        self.transaction(|connection| action(&StateSession { connection }))
    }
    pub(crate) fn read<T>(&self, action: impl FnOnce(&StateSession<'_>) -> Result<T>) -> Result<T> {
        self.read_transaction(|connection| action(&StateSession { connection }))
    }
}
impl StateSession<'_> {
    pub(crate) fn has_event(&self, run: &str, kind: &str) -> Result<bool> {
        Ok(self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM events WHERE run_id=?1 AND kind=?2)",
            params![run, kind],
            |row| row.get(0),
        )?)
    }
    pub(crate) fn get<T: DeserializeOwned>(&self, kind: &str, id: &str) -> Result<Option<T>> {
        Store::tx_get(self.connection, kind, id)
    }
    pub(crate) fn put<T: Serialize>(
        &self,
        kind: &str,
        id: &str,
        run: &str,
        value: &T,
    ) -> Result<()> {
        Store::tx_put(self.connection, kind, id, run, value)
    }
    pub(crate) fn list<T: DeserializeOwned>(&self, kind: &str, run: &str) -> Result<Vec<T>> {
        Store::tx_list(self.connection, kind, run)
    }

    #[cfg(test)]
    pub(crate) fn put_metadata(
        &self,
        kind: &str,
        id: &str,
        run: &str,
        value: &AgentMetadata,
    ) -> Result<()> {
        Store::tx_put_metadata(self.connection, kind, id, run, value)
    }
    pub(crate) fn list_metadata(&self, kind: &str, run: &str) -> Result<Vec<AgentMetadata>> {
        Store::tx_list_metadata(self.connection, kind, run)
    }
    pub(crate) fn agent_snapshot(&self, id: &str) -> Result<Option<AgentSnapshot>> {
        Store::tx_agent_snapshot(self.connection, id)
    }
    pub(crate) fn agent_conversation(&self, id: &str) -> Result<AgentConversation> {
        Store::tx_agent_conversation(self.connection, id)
    }
    pub(crate) fn event(
        &self,
        run: &str,
        agent: Option<&str>,
        kind: &str,
        data: Value,
    ) -> Result<i64> {
        Store::tx_event(self.connection, run, agent, kind, data)
    }
    pub(crate) fn messages(&self, run: &str) -> Result<Vec<Message>> {
        Store::tx_messages(self.connection, run)
    }
    pub(crate) fn message(
        &self,
        run: &str,
        sender: &str,
        recipient: &str,
        content: &str,
        key: Option<&str>,
    ) -> Result<bool> {
        Store::tx_message(self.connection, run, sender, recipient, content, key)
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn enqueue_message(
        &self,
        run: &str,
        sender: &str,
        recipient: &str,
        content: &str,
        key: Option<&str>,
        envelope: &MessageEnvelope,
    ) -> Result<(i64, bool)> {
        Store::tx_enqueue_message(
            self.connection,
            run,
            sender,
            recipient,
            content,
            key,
            envelope,
        )
    }
    pub(crate) fn unread_messages(&self, recipient: &str, interrupting_only: bool) -> Result<i64> {
        Ok(self.connection.query_row("SELECT COUNT(*) FROM messages WHERE recipient=?1 AND acknowledged=0 AND (?2=0 OR COALESCE(json_extract(envelope,'$.kind'),'information')!='information')",params![recipient,interrupting_only],|row|row.get(0))?)
    }
    pub(crate) fn replace_envelope(&self, message: &Message) -> Result<()> {
        self.connection.execute(
            "UPDATE messages SET envelope=?1 WHERE id=?2",
            params![serde_json::to_string(&message.envelope)?, message.id],
        )?;
        Ok(())
    }
    pub(crate) fn stale_mail_before(&self, recipient: &str, generation: u64) -> Result<()> {
        self.connection.execute("UPDATE messages SET acknowledged=1,envelope=json_set(envelope,'$.delivery','stale','$.stale_reason','task reopened in a new generation') WHERE recipient=?1 AND acknowledged=0 AND COALESCE(json_extract(envelope,'$.recipient_generation'),-1)<?2",params![recipient,generation])?;
        Ok(())
    }
    pub(crate) fn contains(&self, kind: &str, id: &str) -> Result<bool> {
        Ok(self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM entities WHERE kind=?1 AND id=?2)",
            params![kind, id],
            |row| row.get(0),
        )?)
    }
    pub(crate) fn completion_facts(&self, run: &str, agent: &str) -> Result<CompletionFacts> {
        #[derive(serde::Deserialize)]
        struct SessionView {
            id: String,
            agent_id: String,
            state: SessionState,
        }
        #[derive(serde::Deserialize)]
        struct JobView {
            id: String,
            status: JobViewStatus,
        }
        #[derive(serde::Deserialize)]
        struct JobViewStatus {
            state: JobState,
        }
        let mut open_sessions = Vec::new();
        for kind in ["debug_session", "interact_session"] {
            for session in self.list::<SessionView>(kind, run)? {
                if !matches!(session.state, SessionState::Closed | SessionState::Lost) {
                    open_sessions.push((kind.into(), session.id, session.agent_id));
                }
            }
        }
        Ok(CompletionFacts {
            unread_messages: self.unread_messages(agent, false)?,
            agents: self.list_metadata("agent", run)?,
            open_sessions,
            unresolved_coverage: self
                .list::<Coverage>("coverage", run)?
                .into_iter()
                .filter(|coverage| coverage.outcome == "needs_follow_up")
                .map(|coverage| coverage.id)
                .collect(),
            running_jobs: self
                .list::<JobView>("job", run)?
                .into_iter()
                .filter(|job| !job.status.state.terminal())
                .map(|job| job.id)
                .collect(),
        })
    }
    #[cfg(test)]
    pub(crate) fn remove(&self, kind: &str, id: &str) -> Result<()> {
        self.connection.execute(
            "DELETE FROM entities WHERE kind=?1 AND id=?2",
            params![kind, id],
        )?;
        Ok(())
    }
}
