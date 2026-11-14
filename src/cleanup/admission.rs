//! Quiesce known local workers without starting a worker or trusting a PID.
use super::*;
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpStream},
    time::Duration,
};

pub(super) fn stop_workers(home: &Path) -> Result<()> {
    for location in locations::inventory(home)?.locations {
        if location.kind != LocationKind::Worker {
            continue;
        }
        let endpoint = location.path.join("endpoint.json");
        if !endpoint.is_file() {
            continue;
        }
        files::safe(&endpoint)?;
        ensure!(
            std::fs::metadata(&endpoint)?.len() < 16384,
            "worker endpoint too large"
        );
        let value: Value = crate::workbench::read_json(&endpoint)?;
        ensure!(
            value["protocol"] == crate::runtime::protocol::PROTOCOL && value["host"] == "127.0.0.1",
            "unrecognized local worker endpoint"
        );
        let port = value["port"]
            .as_u64()
            .filter(|p| *p > 0 && *p <= 65535)
            .context("invalid local worker port")?;
        let address = SocketAddr::from(([127, 0, 0, 1], port as u16));
        let ping = match call(address, &value, "ping") {
            Ok(reply) => reply,
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::ConnectionRefused) =>
            {
                continue;
            }
            Err(error) => {
                return Err(
                    error.context("cannot confirm local worker state; close its sessions first")
                );
            }
        };
        ensure!(
            ping["daemon_pid"] == value["pid"]
                && ping["protocol"] == crate::runtime::protocol::PROTOCOL,
            "local worker identity changed"
        );
        let root = PathBuf::from(ping["state_dir"].as_str().context("worker root missing")?);
        ensure!(
            files::absolute(&root)? == files::absolute(&location.path)?,
            "local worker root changed"
        );
        // The worker rejects this if jobs are still active; owned sessions close there.
        call(address, &value, "worker.shutdown")
            .context("local worker refused shutdown; finish or cancel active work first")?;
        let until = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            match TcpStream::connect_timeout(&address, Duration::from_millis(100)) {
                Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => break,
                _ => ensure!(
                    std::time::Instant::now() < until,
                    "local worker has not stopped; no files deleted"
                ),
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    Ok(())
}
fn call(address: SocketAddr, endpoint: &Value, method: &str) -> Result<Value> {
    let mut socket = TcpStream::connect_timeout(&address, Duration::from_secs(2))?;
    socket.set_read_timeout(Some(Duration::from_secs(5)))?;
    socket.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut request = serde_json::to_vec(
        &json!({"jsonrpc":"2.0","id":1,"method":method,"params":{},"auth":endpoint["auth"]}),
    )?;
    request.push(b'\n');
    socket.write_all(&request)?;
    let mut line = vec![];
    BufReader::new(socket)
        .take(1024 * 1024 + 1)
        .read_until(b'\n', &mut line)?;
    ensure!(
        line.len() <= 1024 * 1024 && line.last() == Some(&b'\n'),
        "invalid local worker reply"
    );
    let reply: Value = serde_json::from_slice(&line)?;
    // Do not expose worker free text or authentication material in uninstall logs.
    ensure!(
        reply["id"] == 1 && reply.get("error").is_none(),
        "local worker rejected maintenance request"
    );
    Ok(reply["result"].clone())
}
