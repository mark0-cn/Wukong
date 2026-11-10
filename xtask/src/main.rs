use std::{env, path::Path, process::Command};

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives in the repository root");
    let result = Command::new("python")
        .args(["-m", "scripts.release.native_package"])
        .args(env::args_os().skip(1))
        .current_dir(root)
        .status();
    match result {
        Ok(status) => std::process::exit(status.code().unwrap_or(1)),
        Err(error) => {
            eprintln!("Cannot start local packaging: {error}. Ensure Python 3.11+ is on PATH.");
            std::process::exit(1);
        }
    }
}
