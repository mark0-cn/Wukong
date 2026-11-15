//! Ownership markers only adopt directories created by this application.
use super::{
    json::{read_json, write_json},
    safety::safe,
};
use anyhow::Result;
use std::path::Path;

/// A marker is written only when Wukong creates a new directory. Existing
/// arbitrary folders are never adopted just because a config names them.
pub fn create_owned_directory(path: &Path) -> Result<()> {
    let new = !path.try_exists()?;
    std::fs::create_dir_all(path)?;
    if new && safe(path).is_ok() {
        write_json(
            &path.join(".wukong-owned.json"),
            &serde_json::json!({"schema_version":1,"application":"io.wukong.desktop"}),
        )?;
    }
    Ok(())
}
pub(crate) fn owns_directory(path: &Path) -> bool {
    let marker = path.join(".wukong-owned.json");
    safe(&marker).is_ok()
        && read_json::<serde_json::Value>(&marker).is_ok_and(|value| {
            value["schema_version"] == 1 && value["application"] == "io.wukong.desktop"
        })
}
