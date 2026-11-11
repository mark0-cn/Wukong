//! Public narration is a projection of model text, never a task or verification result.
use super::*;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReplyState {
    Streaming,
    Complete,
    Interrupted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicReply {
    pub id: String,
    pub run_id: String,
    pub agent_id: String,
    pub generation: u64,
    pub turn: usize,
    pub text: String,
    pub state: ReplyState,
    pub truncated: bool,
    pub revision: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl PublicReply {
    pub fn for_request(agent: &AgentMetadata) -> Self {
        Self {
            id: format!("reply-{}-{}-{}", agent.id, agent.generation, agent.turns),
            run_id: agent.run_id.clone(),
            agent_id: agent.id.clone(),
            generation: agent.generation,
            turn: agent.turns,
            text: String::new(),
            state: ReplyState::Streaming,
            truncated: false,
            revision: 0,
            created_at: now(),
            updated_at: now(),
        }
    }
}
