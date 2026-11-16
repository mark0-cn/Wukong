//! Registered builtin handlers. The catalog owns routing and recovery metadata.
use super::{
    AnalysisService, ExecutionService, analysis_verification, args, documents, environment_service,
    evidence, number, require_document, require_in_run, stable, string_list, target, text,
};
use crate::{
    adapters::binary::{self, StringsQuery},
    application::coordinator::Coordinator,
    domain::*,
    storage::DocumentKind,
};
use anyhow::{Context, Result, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use futures::future::BoxFuture;
use serde_json::{Value, json};
use wukong_worker_protocol::{Method, Request};

mod coordination;
pub(super) use coordination::*;
mod context;
pub(super) use context::*;
mod findings;
pub(super) use findings::*;
mod analysis;
pub(super) use analysis::*;
mod lab;
pub(super) use lab::*;
mod bundle;
pub(super) use bundle::*;
mod execution;
pub(super) use execution::*;
