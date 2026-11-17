//! Managed-SSH environment registration tests. They never connect to a guest.
use super::*;
use crate::application::windows_lab::provision::fixture;
use crate::config::AppConfig;

struct RegistrationFixture {
    directory: tempfile::TempDir,
    state_dir: PathBuf,
    configuration: PathBuf,
    identity_file: PathBuf,
    media: MediaIdentity,
    connection: PublicConnection,
}

fn registration_fixture() -> RegistrationFixture {
    let directory = tempfile::tempdir().unwrap();
    let state_dir = directory.path().join("state");
    let configuration = directory.path().join("wukong.toml");
    std::fs::write(
        &configuration,
        "# operator comment preserved across registration\nstate_dir = \"state\"\n",
    )
    .unwrap();
    let identity_file = directory.path().join("management_ed25519");
    std::fs::write(&identity_file, "CONTROLLED-PRIVATE-KEY-FIXTURE").unwrap();
    let media = fixture::media();
    let connection = media.connection.clone();
    RegistrationFixture {
        state_dir,
        configuration,
        identity_file,
        media: media.identity,
        connection,
        directory,
    }
}

impl RegistrationFixture {
    fn config(&self) -> AppConfig {
        AppConfig::load(Some(&self.configuration)).unwrap()
    }

    fn register(&self, guest_ip: &str, force: bool) -> (EnvRegistration, ConfigWrite) {
        let config = self.config();
        let registration = prepare_environment(
            &config,
            &self.state_dir,
            &self.media,
            &self.connection,
            guest_ip,
            &self.identity_file,
            force,
        )
        .unwrap();
        let written =
            write_configuration(&self.configuration, &registration, &self.state_dir, force)
                .unwrap();
        pin_environment(&self.state_dir, &registration).unwrap();
        (registration, written)
    }
}

#[test]
fn registration_writes_a_managed_ssh_environment_and_public_pin() {
    let fixture = registration_fixture();
    let (registration, written) = fixture.register("192.168.199.129", false);
    assert_eq!(registration.environment_id, "windows-vm");
    assert!(!written.replaced);
    assert!(written.backup.is_file());
    assert_eq!(
        std::fs::read_to_string(&written.backup).unwrap(),
        "# operator comment preserved across registration\nstate_dir = \"state\"\n"
    );
    let text = std::fs::read_to_string(&fixture.configuration).unwrap();
    assert!(text.starts_with("# operator comment preserved across registration"));
    assert!(text.contains("[environments.windows-vm]"));
    assert!(text.contains("backend = \"managed_ssh\""));
    assert!(text.contains("ssh_host = \"192.168.199.129\""));
    assert!(text.contains("default_environment = \"windows-vm\""));

    let reloaded = fixture.config();
    let environment = reloaded.environments.get("windows-vm").unwrap();
    environment.validate().unwrap();
    let ssh = environment.runtime.managed_ssh().unwrap();
    assert_eq!(ssh.installation_id, fixture.connection.installation_id);
    assert_eq!(ssh.user, fixture.media.ssh_user);
    assert_eq!(ssh.port, fixture.media.ssh_port);
    assert_eq!(ssh.identity_file, fixture.identity_file);
    assert!(ssh.known_hosts_file.is_file());
    assert_eq!(
        std::fs::read_to_string(&ssh.known_hosts_file).unwrap(),
        ssh.known_hosts_record().unwrap()
    );
    assert_eq!(reloaded.default_environment.as_deref(), Some("windows-vm"));
    assert_eq!(
        fixture.directory.path().join("state").join("environments"),
        ssh.known_hosts_file.parent().unwrap()
    );
}

#[test]
fn registration_refuses_an_existing_different_environment_without_force() {
    let fixture = registration_fixture();
    fixture.register("192.168.199.129", false);
    let config = fixture.config();
    let error = prepare_environment(
        &config,
        &fixture.state_dir,
        &fixture.media,
        &fixture.connection,
        "192.168.199.130",
        &fixture.identity_file,
        false,
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("--force"), "{error:#}");

    let (registration, written) = fixture.register("192.168.199.130", true);
    assert!(written.replaced);
    assert_eq!(registration.environment_id, "windows-vm");
    let reloaded = fixture.config();
    let ssh = reloaded.environments["windows-vm"]
        .runtime
        .managed_ssh()
        .unwrap();
    assert_eq!(
        reloaded.environments["windows-vm"].name,
        "Windows worker windows-vm"
    );
    assert!(ssh.known_hosts_file.is_file());
}

#[test]
fn registration_rejects_missing_identity_and_mismatched_receipts() {
    let fixture = registration_fixture();
    let config = fixture.config();
    let missing = fixture.directory.path().join("missing-key");
    let error = prepare_environment(
        &config,
        &fixture.state_dir,
        &fixture.media,
        &fixture.connection,
        "192.168.199.129",
        &missing,
        false,
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("identity key is missing"));

    let mut mismatched = fixture.connection.clone();
    mismatched.installation_id = "0".repeat(32);
    let error = prepare_environment(
        &config,
        &fixture.state_dir,
        &fixture.media,
        &mismatched,
        "192.168.199.129",
        &fixture.identity_file,
        false,
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("identity"), "{error:#}");

    let mut mismatched = fixture.connection.clone();
    mismatched.host_fingerprint = "SHA256:wrong".into();
    let error = prepare_environment(
        &config,
        &fixture.state_dir,
        &fixture.media,
        &mismatched,
        "192.168.199.129",
        &fixture.identity_file,
        false,
    )
    .unwrap_err();
    assert!(
        format!("{error:#}").contains("fingerprint") || format!("{error:#}").contains("host key"),
        "{error:#}"
    );
}

#[test]
fn registration_blocks_invalid_hosts_and_missing_configuration() {
    let fixture = registration_fixture();
    let config = fixture.config();
    for host in ["", "host name", "192.168.199.1/24", "192.168.199.1 7"] {
        let error = prepare_environment(
            &config,
            &fixture.state_dir,
            &fixture.media,
            &fixture.connection,
            host,
            &fixture.identity_file,
            false,
        )
        .unwrap_err();
        assert!(!format!("{error:#}").is_empty());
    }
    let (registration, _) = fixture.register("192.168.199.129", false);
    let missing = fixture.directory.path().join("missing.toml");
    let error =
        write_configuration(&missing, &registration, &fixture.state_dir, false).unwrap_err();
    assert!(
        format!("{error:#}").contains("absolute") || format!("{error:#}").contains("existing"),
        "{error:#}"
    );
}

#[test]
fn environment_blocks_never_embed_private_key_material() {
    let fixture = registration_fixture();
    let (registration, _) = fixture.register("192.168.199.129", false);
    let snippet = environment_snippet(&registration, "192.168.199.129").unwrap();
    assert!(snippet.contains("[environments.windows-vm.runtime.managed_ssh]"));
    assert!(snippet.contains("\"192.168.199.129\""));
    assert!(
        snippet.contains(
            &fixture
                .identity_file
                .to_string_lossy()
                .replace('\\', "\\\\")
        )
    );
    assert!(!snippet.contains("CONTROLLED-PRIVATE-KEY-FIXTURE"));
    assert!(!snippet.contains("PRIVATE KEY"));

    let placeholder = placeholder_block(
        &fixture.media,
        &fixture.connection,
        &fixture.state_dir,
        None,
    )
    .unwrap();
    assert!(placeholder.contains("<GUEST_IPV4>"));
    assert!(placeholder.contains("<ABSOLUTE_CONTROLLER_SSH_PRIVATE_KEY>"));
    assert!(placeholder.contains("[environments.windows-vm]"));
}
