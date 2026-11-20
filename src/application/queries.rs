//! Application query facades delegate persistence to the store.
use crate::{
    queries::{
        models::{EvidencePreview, FindingDetails, ReportSnapshot, RunStatus, RunSummary},
        workspace::{WorkspaceRequest, WorkspaceSnapshotReply},
    },
    storage::Store,
};
use anyhow::Result;
pub struct ReportQueries<'a> {
    store: &'a Store,
}
impl<'a> ReportQueries<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }
    pub fn snapshot(&self, run: &str) -> Result<ReportSnapshot> {
        crate::storage::queries::report::snapshot(self.store, run)
    }
}
pub struct AssessmentQueries<'a> {
    store: &'a Store,
}
impl<'a> AssessmentQueries<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }
    pub fn runs(&self) -> Result<Vec<RunSummary>> {
        crate::storage::queries::workspace_delta::runs(self.store)
    }
    pub fn status(&self, run: &str, limit: usize) -> Result<RunStatus> {
        crate::storage::queries::workspace_delta::status(self.store, run, limit)
    }

    /// Read the revision and its matching workspace data in one snapshot.
    pub(crate) fn workspace_snapshot(
        &self,
        epoch: u64,
        request: &WorkspaceRequest,
    ) -> Result<WorkspaceSnapshotReply> {
        crate::storage::queries::workspace_snapshot::read(self.store, epoch, request)
    }
}
pub struct FindingQueries<'a> {
    store: &'a Store,
}
impl<'a> FindingQueries<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }
    pub fn details(&self, run: &str, id: &str) -> Result<FindingDetails> {
        crate::storage::queries::findings::details(self.store, run, id)
    }
    pub fn preview(&self, run: &str, id: &str, offset: u64) -> Result<EvidencePreview> {
        crate::storage::queries::findings::preview(self.store, run, id, offset)
    }
}
#[cfg(test)]
#[path = "query_tests.rs"]
mod tests;
