fn main() -> anyhow::Result<()> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("schema/contract.json");
    let content = serde_json::to_string_pretty(&wukong_worker_protocol::contract())? + "\n";
    if std::env::args().any(|arg| arg == "--check") {
        anyhow::ensure!(
            std::fs::read_to_string(&path)? == content,
            "contract schema is stale; run the protocol export binary"
        );
    } else {
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(path, content)?;
    }
    Ok(())
}
