use super::*;
use serde_json::json;

fn owner() -> Owner {
    Owner {
        identity: Identity {
            protocol: 3,
            installation_id: "a".repeat(32),
            boot_id: format!("windows:{}:40", "b".repeat(32)),
            package_sha256: "c".repeat(64),
        },
        run_id: "run".into(),
        owner_id: "controller".into(),
        epoch: 1,
    }
}
fn put(owner: Owner, data: &[u8]) -> Result<Put> {
    Put::parse(
        owner,
        json!({"path":"targets/sample.bin","data_base64":STANDARD.encode(data),"executable":true})
            .as_object()
            .unwrap(),
    )
}
fn fixture() -> Result<(tempfile::TempDir, Inputs)> {
    let directory = tempfile::tempdir()?;
    let policy = AclPolicy::current_user()?;
    policy.apply(directory.path(), true)?;
    let inputs = Inputs::open(directory.path(), policy)?;
    Ok((directory, inputs))
}

fn reference(put: &Put) -> Result<Reference> {
    let d = &put.declaration;
    Ok(Reference {
        path: d.path.clone(),
        id: digest(d)?,
        sha256: d.sha256.clone(),
        size: d.size,
        executable: d.executable,
    })
}

#[test]
fn native_logical_lookup_needs_completed_exact_owner_and_execution_policy() -> Result<()> {
    let (_root, inputs) = fixture()?;
    let put = put(owner(), b"admitted-bytes")?;
    inputs.prepare(&put)?;
    assert!(
        inputs
            .reference(&owner(), &put.declaration.path, Purpose::Execute)
            .is_err()
    );
    inputs.receipt(&put.declaration)?;
    assert_eq!(
        inputs.reference(&owner(), &put.declaration.path, Purpose::Execute)?,
        reference(&put)?
    );
    let mut changed = owner();
    changed.epoch += 1;
    assert!(
        inputs
            .reference(&changed, &put.declaration.path, Purpose::Execute)
            .is_err()
    );
    for path in [
        "C:/Windows/System32/cmd.exe",
        "files/../outside.exe",
        "files/other.exe",
    ] {
        assert!(inputs.reference(&owner(), path, Purpose::Execute).is_err());
    }
    let mut data = put.declaration.clone();
    data.path = "files/data.bin".into();
    data.executable = false;
    let data = Put {
        declaration: data,
        data: put.data.clone(),
    };
    inputs.prepare(&data)?;
    inputs.receipt(&data.declaration)?;
    assert!(
        inputs
            .reference(&owner(), &data.declaration.path, Purpose::Execute)
            .is_err()
    );
    inputs.reference(&owner(), &data.declaration.path, Purpose::Inspect)?;
    Ok(())
}

#[test]
fn native_resource_ingestion_requires_a_completed_receipt_and_pins_the_verified_handle()
-> Result<()> {
    use std::io::Read;
    let (_directory, inputs) = fixture()?;
    let put = put(owner(), b"only-controller-admitted-bytes")?;
    let reference = reference(&put)?;
    inputs.prepare(&put)?;
    assert!(
        inputs
            .acquire(&owner(), &reference, Purpose::Execute)
            .is_err()
    );
    inputs.receipt(&put.declaration)?;
    let mut input = inputs.acquire(&owner(), &reference, Purpose::Execute)?;
    assert_eq!(input.reference(), &reference);
    let blob = inputs
        .directory(&put.declaration)?
        .join(format!("{}.data", digest(&put.declaration.path)?));
    assert!(fs::OpenOptions::new().write(true).open(&blob).is_err());
    assert!(fs::rename(&blob, blob.with_extension("renamed")).is_err());
    // An ordinary read-only retry may still validate the immutable source.
    inputs.receipt(&put.declaration)?;
    let mut bytes = Vec::new();
    input.read_to_end(&mut bytes)?;
    assert_eq!(bytes, b"only-controller-admitted-bytes");
    drop(input);
    let _writer = fs::OpenOptions::new().write(true).open(&blob)?;
    Ok(())
}

#[test]
fn resource_input_selectors_cannot_change_owner_content_or_executable_policy() -> Result<()> {
    let (_directory, inputs) = fixture()?;
    let mut put = put(owner(), b"data-only-input")?;
    put.declaration.executable = false;
    inputs.prepare(&put)?;
    inputs.receipt(&put.declaration)?;
    let expected = reference(&put)?;
    assert!(
        inputs
            .acquire(&owner(), &expected, Purpose::Inspect)
            .is_ok()
    );
    assert!(
        inputs
            .acquire(&owner(), &expected, Purpose::Execute)
            .is_err()
    );
    for changed in [
        Reference {
            id: "f".repeat(64),
            ..expected.clone()
        },
        Reference {
            sha256: "e".repeat(64),
            ..expected.clone()
        },
        Reference {
            size: expected.size + 1,
            ..expected.clone()
        },
        Reference {
            executable: true,
            ..expected.clone()
        },
        Reference {
            path: "files/../metadata/authority.json".into(),
            ..expected.clone()
        },
        Reference {
            path: "C:/Windows/System32/config/SAM".into(),
            ..expected.clone()
        },
        Reference {
            path: "files/NUL".into(),
            ..expected.clone()
        },
    ] {
        assert!(
            inputs
                .acquire(&owner(), &changed, Purpose::Inspect)
                .is_err()
        );
    }
    for changed in [
        Owner {
            run_id: "another-run".into(),
            ..owner()
        },
        Owner {
            owner_id: "another-controller".into(),
            ..owner()
        },
        Owner {
            epoch: 2,
            ..owner()
        },
        Owner {
            identity: Identity {
                boot_id: format!("windows:{}:41", "b".repeat(32)),
                ..owner().identity
            },
            ..owner()
        },
    ] {
        assert!(
            inputs
                .acquire(&changed, &expected, Purpose::Inspect)
                .is_err()
        );
        assert!(!inputs.root.join(digest(&changed)?).exists());
    }
    Ok(())
}

#[test]
fn resource_input_acquisition_rechecks_private_bytes_and_rejects_links() -> Result<()> {
    let (directory, inputs) = fixture()?;
    let put = put(owner(), b"original-native-input")?;
    inputs.prepare(&put)?;
    inputs.receipt(&put.declaration)?;
    let reference = reference(&put)?;
    let blob = inputs
        .directory(&put.declaration)?
        .join(format!("{}.data", digest(&put.declaration.path)?));
    let alias = directory.path().join("second-link");
    fs::hard_link(&blob, &alias)?;
    assert!(
        inputs
            .acquire(&owner(), &reference, Purpose::Execute)
            .is_err()
    );
    fs::remove_file(&alias)?;
    fs::write(&blob, b"modified-native-input")?;
    assert!(
        inputs
            .acquire(&owner(), &reference, Purpose::Execute)
            .is_err()
    );
    Ok(())
}

#[test]
fn ordinary_processes_cannot_use_the_system_resource_input_entry() -> Result<()> {
    let root = tempfile::tempdir()?;
    let put = put(owner(), b"inert")?;
    let error = crate::artifacts::resource_input::open(
        &root.path().join("absent-metadata"),
        &owner(),
        &reference(&put)?,
        Purpose::Execute,
    )
    .err()
    .context("ordinary token entered System input API")?;
    assert!(error.to_string().contains("System"));
    assert_eq!(fs::read_dir(root.path())?.count(), 0);
    Ok(())
}

#[test]
fn direct_sdk_publication_handles_members_beyond_the_legacy_windows_path_limit() -> Result<()> {
    let root = tempfile::tempdir()?;
    let policy = AclPolicy::current_user()?;
    policy.apply(root.path(), true)?;
    let metadata = root
        .path()
        .join(format!("long-metadata-{}", "x".repeat(100)));
    policy.create_directory(&metadata)?;
    let inputs = Inputs::open(&metadata, policy)?;
    let put = put(owner(), b"known long-path fixture")?;
    let directory = inputs.directory(&put.declaration)?;
    let member = directory.join(format!(
        "{}.declaration.json",
        digest(&put.declaration.path)?
    ));
    assert!(member.to_string_lossy().len() > 260);
    inputs.prepare(&put)?;
    let result = inputs.receipt(&put.declaration)?;
    assert_eq!(result["native_input"]["sha256"], result["sha256"]);
    Ok(())
}
#[test]
fn native_bytes_and_receipts_are_bound_to_the_controller_owner_and_survive_reopen() -> Result<()> {
    let (directory, inputs) = fixture()?;
    let first = put(owner(), b"original-controller-bytes")?;
    inputs.prepare(&first)?;
    let scope = inputs.directory(&first.declaration)?;
    let key = digest(&first.declaration.path)?;
    assert!(!scope.join(format!("{key}.receipt.json")).exists());
    let result = inputs.receipt(&first.declaration)?;
    assert_eq!(result["native_input"]["epoch"], 1);
    assert_eq!(
        result["native_input"]["identity"],
        serde_json::to_value(owner().identity)?
    );
    assert!(result["native_input"].get("token").is_none());
    drop(inputs);
    let inputs = Inputs::open(directory.path(), AclPolicy::current_user()?)?;
    inputs.prepare(&first)?;
    let repeated = inputs.receipt(&first.declaration)?;
    assert_eq!(result, repeated);
    for other in [
        Owner {
            run_id: "other".into(),
            ..owner()
        },
        Owner {
            owner_id: "other".into(),
            ..owner()
        },
        Owner {
            epoch: 2,
            ..owner()
        },
    ] {
        let other = put(other, b"original-controller-bytes")?;
        inputs.prepare(&other)?;
        let observation = inputs.receipt(&other.declaration)?;
        assert_ne!(
            observation["native_input"]["id"],
            result["native_input"]["id"]
        );
        assert_ne!(inputs.directory(&other.declaration)?, scope);
    }
    Ok(())
}
#[test]
fn changed_bytes_never_create_an_admitted_receipt() -> Result<()> {
    let (_directory, inputs) = fixture()?;
    let first = put(owner(), b"controller-bytes")?;
    inputs.prepare(&first)?;
    let scope = inputs.directory(&first.declaration)?;
    let key = digest(&first.declaration.path)?;
    assert!(inputs.prepare(&put(owner(), b"changed-request")?).is_err());
    fs::write(scope.join(format!("{key}.data")), b"modified-content")?;
    assert!(inputs.prepare(&first).is_err());
    assert!(inputs.receipt(&first.declaration).is_err());
    assert!(!scope.join(format!("{key}.receipt.json")).exists());
    Ok(())
}
#[test]
fn portable_path_and_exact_executable_policy_are_validated_without_reading_worker_paths()
-> Result<()> {
    for path in [
        "../metadata/authority.json",
        "C:/Windows/System32/config/SAM",
        "a\\b",
        "/absolute",
        "file:stream",
        "NUL",
        "a/../b",
        "trailing.",
        "a//b",
    ] {
        let params = json!({"path":path,"data_base64":"AA=="});
        assert!(
            Put::parse(owner(), params.as_object().unwrap()).is_err(),
            "{path}"
        );
    }
    for executable in [json!(null), json!(1), json!("true")] {
        let params = json!({"path":"file.bin","data_base64":"AA==","executable":executable});
        assert!(Put::parse(owner(), params.as_object().unwrap()).is_err());
    }
    let params = json!({"path":"file.bin","data_base64":"not-base64"});
    assert!(Put::parse(owner(), params.as_object().unwrap()).is_err());
    Ok(())
}

#[test]
fn a_second_link_to_native_bytes_invalidates_admission() -> Result<()> {
    let (directory, inputs) = fixture()?;
    let input = put(owner(), b"single-native-identity")?;
    inputs.prepare(&input)?;
    let scope = inputs.directory(&input.declaration)?;
    let blob = scope.join(format!("{}.data", digest(&input.declaration.path)?));
    let alias = directory.path().join("fixture-alias.data");
    fs::hard_link(&blob, &alias)?;
    assert!(inputs.prepare(&input).is_err());
    assert!(inputs.receipt(&input.declaration).is_err());
    fs::remove_file(&alias)?;
    inputs.prepare(&input)?;
    inputs.receipt(&input.declaration)?;
    Ok(())
}
