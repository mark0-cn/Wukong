use super::*;
use crate::adapters::worker::protocol::TransportFailure;
use std::fs;

#[tokio::test]
async fn reconnect_keeps_identity_and_never_replays_a_timed_out_request() -> Result<()> {
    let fixture = tempfile::tempdir()?;
    let bridge = fixture.path().join("bridge.py");
    let state = fixture.path().join("worker");
    fs::write(
        &bridge,
        r#"
import json
from pathlib import Path
import sys
import time

root = Path(sys.argv[sys.argv.index("--state") + 1])
for line in sys.stdin:
    request = json.loads(line)
    if request["method"] == "ping":
        boot = (root / "boot").read_text()
        result = {"identity": {"protocol": 3, "installation_id": "fixture",
                               "boot_id": boot, "package_sha256": "a" * 64}}
    elif request["method"] == "doctor":
        result = {"ready": True}
    elif request["method"] == "file.put":
        with (root / "writes").open("a") as effects:
            effects.write(request["params"]["request_id"] + "\n")
        time.sleep(30)
        result = {}
    else:
        raise AssertionError(request["method"])
    print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
"#,
    )?;
    fs::create_dir(&state)?;
    fs::write(state.join("boot"), "boot-1")?;
    let client = RuntimeClient::connect(RuntimeConfig {
        transport: TransportConfig::TrustedLocal(TrustedLocalConfig {
            state_dir: state.clone(),
            bridge_path: bridge,
            ..Default::default()
        }),
        request_timeout_secs: 2,
    })
    .await?;
    let failure = client
        .call("file.put", json!({"request_id": "fixture-write"}))
        .await
        .unwrap_err();
    assert!(
        failure
            .downcast_ref::<TransportFailure>()
            .is_some_and(|error| error.timed_out)
    );
    assert_eq!(client.doctor().await?, json!({"ready": true}));
    assert_eq!(fs::read_to_string(state.join("writes"))?, "fixture-write\n");

    client.close().await?;
    fs::write(state.join("boot"), "boot-2")?;
    let failure = client.doctor().await.unwrap_err();
    assert!(format!("{failure:#}").contains("identity changed"));
    assert_eq!(fs::read_to_string(state.join("writes"))?, "fixture-write\n");
    Ok(())
}
