fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto/broker.proto");
    let descriptors = protox::compile(["proto/broker.proto"], ["proto"])?;
    let mut config = prost_build::Config::new();
    for message in ["Identity", "Health"] {
        config.type_attribute(message, "#[derive(serde::Serialize, serde::Deserialize)]");
        config.type_attribute(message, "#[serde(deny_unknown_fields)]");
    }
    config.compile_fds(descriptors)?;
    println!("cargo:rerun-if-changed=proto/session.proto");
    let mut sessions = prost_build::Config::new();
    for message in ["Inspect", "Control"] {
        sessions.type_attribute(message, "#[derive(serde::Deserialize)]");
        sessions.type_attribute(message, "#[serde(deny_unknown_fields)]");
        sessions.field_attribute(format!("{message}.timeout_seconds"), "#[serde(default = \"crate::session_commands::default_timeout\", rename = \"timeout_secs\")]");
    }
    sessions.field_attribute(
        "Inspect.kind",
        "#[serde(deserialize_with = \"crate::session_commands::inspection\")]",
    );
    sessions.field_attribute(
        "Control.action",
        "#[serde(deserialize_with = \"crate::session_commands::action\")]",
    );
    sessions.field_attribute(
        "Control.breakpoint_id",
        "#[serde(default, deserialize_with = \"crate::session_commands::breakpoint_id\")]",
    );
    sessions.compile_fds(protox::compile(["proto/session.proto"], ["proto"])?)?;
    Ok(())
}
