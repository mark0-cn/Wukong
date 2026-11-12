use super::*;
use crate::{
    accounts,
    processes::{Job, OwnedProcess, Spawn},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::Path};

fn identity(id: &str) -> Identity {
    Identity {
        protocol: 2,
        installation_id: id.into(),
        boot_id: format!("windows:{}:13", "b".repeat(32)),
        package_sha256: "c".repeat(64),
    }
}
#[test]
fn native_broker_fixture_entry() -> Result<()> {
    let Ok(installation) = std::env::var("WUKONG_BROKER_FIXTURE_INSTALLATION") else {
        return Ok(());
    };
    let parent = std::env::var("WUKONG_BROKER_FIXTURE_PARENT")?.parse()?;
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(health(&installation, parent));
    let value = match result {
        Ok(value) => value,
        Err(error) => json!({"error":format!("{error:#}")}),
    };
    fs::write(
        std::env::current_dir()?.join("client.json"),
        serde_json::to_vec(&value)?,
    )?;
    Ok(())
}
fn child(id: &str, directory: &Path) -> Result<OwnedProcess> {
    let executable = std::env::current_exe()?;
    let input = fs::File::open("NUL")?;
    let log = fs::File::create(directory.join("client.log"))?;
    let mut environment = BTreeMap::new();
    for key in ["SystemRoot", "PATH", "TEMP", "TMP"] {
        if let Some(value) = std::env::var_os(key) {
            environment.insert(key.into(), value);
        }
    }
    environment.insert("WUKONG_BROKER_FIXTURE_INSTALLATION".into(), id.into());
    environment.insert(
        "WUKONG_BROKER_FIXTURE_PARENT".into(),
        std::process::id().to_string().into(),
    );
    Spawn {
        executable: &executable,
        arguments: &[
            "--exact".into(),
            "broker::tests::native_broker_fixture_entry".into(),
            "--nocapture".into(),
        ],
        directory,
        environment: &environment,
        log: &log,
        input: &input,
    }
    .inert_fixture()
}
async fn result(child: &OwnedProcess, directory: &Path) -> Result<Value> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !child.exited() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    ensure!(
        child.exit_code()? == Some(0),
        "inert native broker fixture failed"
    );
    Ok(serde_json::from_slice(&fs::read(
        directory.join("client.json"),
    )?)?)
}

#[tokio::test]
async fn real_worker_job_and_sid_admit_broker_health_but_never_execution() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let id = hex::encode(accounts::random_bytes::<16>()?);
    let sid = security::current_user_sid()?;
    let server = pipe_transport::broker_fixture_server(&id, &sid)?;
    let child = child(&id, directory.path())?;
    let observer = child.job().observer()?;
    assert!(observer.contains(child.pid())?);
    assert!(!observer.contains(std::process::id())?);
    let expected = identity(&id);
    let broker = Broker::new(expected.clone(), sid.clone(), observer)?;
    tokio::time::timeout(Duration::from_secs(5), server.connect()).await??;
    broker.serve(server).await?;
    let value = result(&child, directory.path()).await?;
    assert_eq!(value["identity"], serde_json::to_value(expected)?);
    assert_eq!(value["production_execution"], false);
    assert_eq!(security::current_user_sid()?, sid);
    child.stop(Duration::from_secs(5))?;
    Ok(())
}

#[tokio::test]
async fn same_sid_from_another_job_and_wrong_sid_are_rejected() -> Result<()> {
    for wrong_sid in [false, true] {
        let directory = tempfile::tempdir()?;
        let id = hex::encode(accounts::random_bytes::<16>()?);
        let sid = security::current_user_sid()?;
        let server = pipe_transport::broker_fixture_server(&id, &sid)?;
        let child = child(&id, directory.path())?;
        let foreign = Job::new()?;
        let observer = if wrong_sid {
            child.job().observer()?
        } else {
            foreign.observer()?
        };
        let expected_sid = if wrong_sid {
            "S-1-5-21-11-22-33-1001".into()
        } else {
            sid.clone()
        };
        let broker = Broker::new(identity(&id), expected_sid, observer)?;
        tokio::time::timeout(Duration::from_secs(5), server.connect()).await??;
        let error = broker.serve(server).await.unwrap_err().to_string();
        assert!(
            error.contains(if wrong_sid {
                "not the metadata worker"
            } else {
                "outside the current worker Job"
            }),
            "{error}"
        );
        assert!(result(&child, directory.path()).await?["error"].is_string());
        assert_eq!(security::current_user_sid()?, sid);
        child.stop(Duration::from_secs(5))?;
    }
    Ok(())
}

#[tokio::test]
async fn client_checks_kernel_server_pid_and_broker_frames_are_smaller_than_relay_frames()
-> Result<()> {
    let id = hex::encode(accounts::random_bytes::<16>()?);
    let sid = security::current_user_sid()?;
    let server = pipe_transport::broker_fixture_server(&id, &sid)?;
    let (_accepted, client) = tokio::join!(
        server.connect(),
        pipe_transport::connect_broker(&id, std::process::id() + 1)
    );
    assert!(
        client
            .unwrap_err()
            .to_string()
            .contains("trusted service parent PID")
    );
    let oversized = vec![b'x'; FRAME_LIMIT + 1];
    let mut reader = BufReader::new(oversized.as_slice());
    assert!(
        pipe_transport::read_bounded_frame(&mut reader, FRAME_LIMIT)
            .await
            .is_err()
    );
    for invalid in [
        b"{\"schema\":1,\"operation\":\"launch\"}".as_slice(),
        b"{\"schema\":1,\"operation\":\"health\",\"pid\":4}".as_slice(),
    ] {
        assert!(serde_json::from_slice::<Request>(invalid).is_err());
    }
    Ok(())
}
