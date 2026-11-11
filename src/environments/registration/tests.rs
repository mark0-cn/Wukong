use super::*;
use base64::Engine;

fn input(root: &Path) -> Result<WindowsConnection> {
    let mut blob = vec![0, 0, 0, 11];
    blob.extend_from_slice(b"ssh-ed25519");
    blob.extend_from_slice(&[0, 0, 0, 32]);
    blob.extend_from_slice(&[7; 32]);
    let key = SshPublicKey {
        algorithm: "ssh-ed25519".into(),
        key: base64::engine::general_purpose::STANDARD.encode(blob),
    };
    let fingerprint = key.fingerprint()?;
    let identity = root.join("existing-identity");
    fs::write(&identity, "CONTROLLED-PRIVATE-CANARY-NOT-AN-SSH-KEY")?;
    Ok(WindowsConnection {
        connection_json: json!({"schema":1,"installed":true,
        "installation_id":"a".repeat(32),"environment_id":"windows","generation":1,
        "manifest_sha256":"b".repeat(64),"ssh_user":format!("wkgm{}", "a".repeat(16)),
        "ssh_port":22,"host_key":key,"host_fingerprint":fingerprint,
        "production_isolation_verified":true})
        .to_string(),
        name: "Windows VM".into(),
        host: "192.0.2.1".into(),
        identity_file: identity,
        confirmed_fingerprint: fingerprint,
        make_default: true,
    })
}

#[test]
fn registration_is_lazy_and_imports_only_a_public_pin_without_reading_the_private_key() -> Result<()>
{
    let root = tempfile::tempdir()?;
    let input = input(root.path())?;
    let home = root.path().join("profile");
    fs::create_dir(&home)?;
    let (id, value) = prepare(&home, &input, &BTreeMap::new())?;
    assert_eq!(id, "windows");
    assert!(!value.to_string().contains("CONTROLLED-PRIVATE-CANARY"));
    assert!(value.pointer("/production_isolation_verified").is_none());
    let environment: EnvironmentConfig = serde_json::from_value(value)?;
    let ssh = environment.runtime.managed_ssh.as_ref().unwrap();
    assert!(!ssh.known_hosts_file.exists());
    let expected = ssh.known_hosts_record()?;
    let path = ssh.known_hosts_file.clone();
    let environments = BTreeMap::from([(id, environment)]);
    assert_eq!(prepare_pin_files(&home, &environments)?, vec![path.clone()]);
    assert_eq!(fs::read(&path)?, expected.as_bytes());
    assert_eq!(prepare_pin_files(&home, &environments)?, vec![path.clone()]);
    fs::write(&path, "independently edited public settings")?;
    assert!(prepare_pin_files(&home, &environments).is_err());
    assert_eq!(
        fs::read_to_string(&path)?,
        "independently edited public settings"
    );
    assert_eq!(
        fs::read_to_string(&input.identity_file)?,
        "CONTROLLED-PRIVATE-CANARY-NOT-AN-SSH-KEY"
    );
    Ok(())
}

#[test]
fn registration_refuses_unconfirmed_keys_malformed_receipts_and_replacement_ids() -> Result<()> {
    let root = tempfile::tempdir()?;
    let mut input = input(root.path())?;
    let original = input.confirmed_fingerprint.clone();
    input.confirmed_fingerprint.clear();
    assert!(prepare(root.path(), &input, &BTreeMap::new()).is_err());
    input.confirmed_fingerprint = original;
    let (id, value) = prepare(root.path(), &input, &BTreeMap::new())?;
    let existing = BTreeMap::from([(id, serde_json::from_value(value)?)]);
    assert!(prepare(root.path(), &input, &existing).is_err());
    input.host = "user@host -oProxyCommand=untrusted".into();
    assert!(prepare(root.path(), &input, &BTreeMap::new()).is_err());
    input.host = "192.0.2.1".into();
    let receipt = input.connection_json.clone();
    for invalid in [
        "{}".to_string(),
        "x".repeat(16385),
        receipt.replace("ssh-ed25519", "ssh-rsa"),
        receipt.replace("\"schema\":1", "\"schema\":2"),
    ] {
        input.connection_json = invalid;
        assert!(prepare(root.path(), &input, &BTreeMap::new()).is_err());
    }
    Ok(())
}

#[test]
fn only_generated_host_pin_paths_are_prepared() -> Result<()> {
    let root = tempfile::tempdir()?;
    let input = input(root.path())?;
    let (id, value) = prepare(root.path(), &input, &BTreeMap::new())?;
    let mut environment: EnvironmentConfig = serde_json::from_value(value)?;
    let external = root.path().join("user-selected-known-hosts");
    environment
        .runtime
        .managed_ssh
        .as_mut()
        .unwrap()
        .known_hosts_file = external.clone();
    assert!(prepare_pin_files(root.path(), &BTreeMap::from([(id, environment)]))?.is_empty());
    assert!(!external.exists());
    Ok(())
}
