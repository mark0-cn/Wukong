//! Runs an owned cleanup host and returns its original protected receipt; no ledger access.
use crate::{
    host::context::RuntimeContext,
    isolation::{processes::Spawn, security::AclPolicy},
    resources::resource_recovery::Request,
};
use anyhow::{Result, ensure};
use std::{
    fs,
    time::{Duration, Instant},
};
pub(super) fn observe(context: RuntimeContext, request: Request) -> Result<Vec<u8>> {
    let suffix = format!("recovery-{}", request.attempt);
    let path = context
        .artifacts
        .request_path(&request.resource.id, &format!("{suffix}.json"))?;
    let policy = AclPolicy::private_vm();
    crate::artifacts::io::publish_json(&path, &request, &policy)?;
    let log = policy.create_file(
        &context
            .artifacts
            .request_path(&request.resource.id, &format!("{suffix}.log"))?,
    )?;
    let input = fs::File::open("NUL")?;
    let environment = crate::isolation::environment::system(&context.root)?;
    let host = Spawn {
        executable: &context.executable,
        arguments: &[
            "resource-recover".into(),
            context.installation.clone().into(),
            context
                .root
                .join("metadata/deployment.json")
                .into_os_string(),
            path.into_os_string(),
        ],
        directory: &context.root,
        environment: &environment,
        log: &log,
        input: &input,
    }
    .system_control()?;
    let deadline = Instant::now() + Duration::from_secs(90);
    while !host.exited() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let code = host.stop(Duration::from_secs(10))?;
    ensure!(
        code == 0,
        "native cleanup host failed for resource {}, attempt {}, exit code {code}; retain the resource blocker",
        request.resource.id,
        request.attempt
    );
    let path = context
        .artifacts
        .result_path(&request.resource.id, &format!("{suffix}.json"))?;
    policy.verify(&path, false)?;
    crate::artifacts::io::read_bytes(&path, 65536)
}
