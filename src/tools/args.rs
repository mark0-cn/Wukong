//! Typed tool inputs. Rig derives the model schema from these same types.
use crate::domain::{DependencyPolicy, FindingStage, Severity, TaskContract, TaskResult};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

macro_rules! choices {
    ($name:ident { $($variant:ident),* $(,)? }) => {
        #[derive(Serialize, Deserialize, JsonSchema)]
        #[serde(rename_all = "snake_case")]
        pub(in crate::tools) enum $name { $($variant),* }
    };
}

mod common;
pub(super) use common::*;
mod coordination;
pub(super) use coordination::*;
mod context;
pub(super) use context::*;
mod findings;
pub(super) use findings::*;
mod analysis;
pub(super) use analysis::*;
mod execution;
pub(super) use execution::*;
