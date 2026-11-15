use super::*;
use crate::domain::Budget;
#[test]
fn cli_and_workspace_share_summary_contract_without_saved_configuration() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let target = directory.path().join("sample");
    std::fs::write(&target, b"fixture")?;
    let store = Store::open(directory.path().join("state"))?;
    let run = crate::application::coordinator::Coordinator::new(store.clone()).create_run(
        &target,
        "fixture",
        Budget::default(),
        serde_json::json!({"internal_setting":"excluded"}),
    )?;
    let status = AssessmentQueries::new(&store).status(&run.id, 5)?;
    let workspace = WorkspaceQueries::default().read(&store, Some(&run.id), None, 5)?;
    assert_eq!(status.run.id, workspace.runs[0].value.as_ref().unwrap().id);
    assert_eq!(
        status.agents[0].id,
        workspace.agents[0].value.as_ref().unwrap().id
    );
    assert!(serde_json::to_value(status)?["run"].get("config").is_none());
    assert_eq!(ReportQueries::new(&store).snapshot(&run.id)?.run.id, run.id);
    Ok(())
}
