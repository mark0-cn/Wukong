//! Bounded discovery checks shared by the dedicated-VM acceptance fixtures.
use crate::runtime::RuntimeClient;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

pub(super) async fn inventory(client: &RuntimeClient) -> Result<Value> {
    let mut jobs = Vec::new();
    let mut after: Option<String> = None;
    for _ in 0..32 {
        let page = client
            .call("job.list", json!({"limit":2,"after":after}))
            .await?;
        let page = page
            .as_array()
            .context("native job inventory is not an array")?;
        ensure!(page.len() <= 2, "native job page exceeded its limit");
        if page.is_empty() {
            break;
        }
        for job in page {
            let id = job["job_id"].as_str().context("listed job ID is absent")?;
            ensure!(
                after.as_deref().is_none_or(|previous| id > previous),
                "job cursor did not advance exclusively"
            );
            after = Some(id.to_owned());
            jobs.push(job.clone());
        }
    }
    ensure!(jobs.len() < 64, "fixture job inventory exceeded its bound");
    let mut resources = Vec::new();
    after = None;
    for _ in 0..32 {
        let page = client
            .call("resource.list", json!({"limit":2,"after":after}))
            .await?;
        ensure!(
            page["schema"] == 1 && page["production_execution"] == false,
            "native inventory envelope differs"
        );
        let records = page["resources"]
            .as_array()
            .context("native resource page is absent")?;
        ensure!(
            records.len() <= 2,
            "native resource page exceeded its limit"
        );
        for record in records {
            let id = record["resource_id"]
                .as_str()
                .context("listed resource ID is absent")?;
            ensure!(
                after.as_deref().is_none_or(|previous| id > previous)
                    && record["kind"] == "process"
                    && record["specification_sha256"]
                        .as_str()
                        .is_some_and(|hash| hash.len() == 64)
                    && record.get("specification").is_none(),
                "resource identity/kind/bounded summary differs"
            );
            after = Some(id.to_owned());
            resources.push(record.clone());
        }
        if page["next_after"].is_null() {
            break;
        }
        ensure!(
            !records.is_empty() && page["next_after"].as_str() == after.as_deref(),
            "resource cursor is absent or changed"
        );
    }
    ensure!(
        resources.len() < 64 && jobs.len() == resources.len(),
        "native process inventory lost or duplicated records"
    );
    for (job, resource) in jobs.iter().zip(&resources) {
        ensure!(
            job["job_id"] == resource["resource_id"],
            "job/resource inventories disagree"
        );
    }
    Ok(json!({"jobs":jobs,"resources":resources}))
}
