//! Thin adapters for bundle services.
use super::*;
use crate::application::tools::bundle::{self, BundleService};
pub(in crate::application::tools) fn handle_bundle_manifest<'a>(
    service: &'a BundleService,
    agent: &'a AgentMetadata,
    call: &'a str,
    name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    bundle::handle_bundle_manifest(service, agent, call, name, args)
}
pub(in crate::application::tools) fn handle_bundle_evidence<'a>(
    service: &'a BundleService,
    agent: &'a AgentMetadata,
    call: &'a str,
    name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    bundle::handle_bundle_evidence(service, agent, call, name, args)
}
