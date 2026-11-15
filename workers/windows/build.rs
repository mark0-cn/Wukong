fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto/session.proto");
    let mut sessions = prost_build::Config::new();
    for message in ["Inspect", "Control"] {
        sessions.type_attribute(message, "#[derive(serde::Deserialize)]");
        sessions.type_attribute(message, "#[serde(deny_unknown_fields)]");
        sessions.field_attribute(format!("{message}.timeout_seconds"), "#[serde(default = \"crate::executors::session_commands::default_timeout\", rename = \"timeout_secs\")]");
    }
    sessions.field_attribute(
        "Inspect.kind",
        "#[serde(deserialize_with = \"crate::executors::session_commands::inspection\")]",
    );
    sessions.field_attribute(
        "Control.action",
        "#[serde(deserialize_with = \"crate::executors::session_commands::action\")]",
    );
    sessions.field_attribute(
        "Control.breakpoint_id",
        "#[serde(default, deserialize_with = \"crate::executors::session_commands::breakpoint_id\")]",
    );
    sessions.compile_fds(protox::compile(["proto/session.proto"], ["proto"])?)?;
    Ok(())
}
