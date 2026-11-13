fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto/broker.proto");
    let descriptors = protox::compile(["proto/broker.proto"], ["proto"])?;
    let mut config = prost_build::Config::new();
    for message in ["Identity", "Health"] {
        config.type_attribute(message, "#[derive(serde::Serialize, serde::Deserialize)]");
        config.type_attribute(message, "#[serde(deny_unknown_fields)]");
    }
    config.compile_fds(descriptors)?;
    Ok(())
}
