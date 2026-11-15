//! Application query facades delegate persistence to the store.
use crate::{queries::models::*, store::Store};
use anyhow::Result;
pub struct ReportQueries<'a> {
    store: &'a Store,
}
impl<'a> ReportQueries<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }
    pub fn snapshot(&self, run: &str) -> Result<ReportSnapshot> {
        crate::store::report_queries::snapshot(self.store, run)
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
        crate::store::workspace::runs(self.store)
    }
    pub fn status(&self, run: &str, limit: usize) -> Result<RunStatus> {
        crate::store::workspace::status(self.store, run, limit)
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
        crate::store::finding_queries::details(self.store, run, id)
    }
    pub fn preview(&self, run: &str, id: &str, offset: u64) -> Result<EvidencePreview> {
        crate::store::finding_queries::preview(self.store, run, id, offset)
    }
}
#[cfg(test)]
#[path = "query_tests.rs"]
mod tests;
