//! Transactional task coordination; use cases share the same Store transaction boundary.
mod completion;
mod messages;
mod runs;
mod tasks;
mod waits;

use crate::{domain::*, store::Store};
use anyhow::{Context, Result, bail, ensure};
use chrono::Duration;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::HashSet, fs, path::Path};

#[derive(Clone)]
pub struct Coordinator {
    pub store: Store,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChildRequest {
    pub name: String,
    pub role: Role,
    pub task: String,
    pub scope: String,
    /// Omitted means inherit the parent's environment.
    #[serde(default)]
    pub environment_id: Option<String>,
    #[serde(default)]
    pub contract: TaskContract,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default = "yes")]
    pub inherit_context: bool,
    /// Predecessor agents evaluated using dependency_policy before admission.
    #[serde(default)]
    pub wait_for_predecessors: Vec<String>,
    #[serde(default)]
    pub dependency_policy: DependencyPolicy,
}
fn yes() -> bool {
    true
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnResult {
    pub created: bool,
    pub agent: AgentSnapshot,
}

impl Coordinator {
    pub fn new(store: Store) -> Self {
        Self { store }
    }
}
