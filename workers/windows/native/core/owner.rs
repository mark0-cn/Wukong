use serde::{Deserialize, Serialize};
use wukong_worker_protocol::WorkerIdentity as Identity;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Owner {
    pub identity: Identity,
    pub run_id: String,
    pub owner_id: String,
    pub epoch: u64,
}
