use super::*;
use serde_json::json;

fn owner() -> Owner {
    Owner {
        identity: Identity {
            protocol: 2,
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
fn reply(put: &Put) -> Value {
    json!({"path":put.declaration.path,"sha256":put.declaration.sha256,"size":put.declaration.size})
}
fn fixture() -> Result<(tempfile::TempDir, Inputs)> {
    let directory = tempfile::tempdir()?;
    let policy = AclPolicy::current_user()?;
    policy.apply(directory.path(), true)?;
    let inputs = Inputs::open(directory.path(), policy)?;
    Ok((directory, inputs))
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
    let mut result = reply(&put);
    inputs.complete(&put, &mut result)?;
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
    let mut result = reply(&first);
    inputs.complete(&first, &mut result)?;
    assert_eq!(result["native_input"]["epoch"], 1);
    assert_eq!(
        result["native_input"]["identity"],
        serde_json::to_value(owner().identity)?
    );
    assert!(result["native_input"].get("token").is_none());
    drop(inputs);
    let inputs = Inputs::open(directory.path(), AclPolicy::current_user()?)?;
    inputs.prepare(&first)?;
    let mut repeated = reply(&first);
    inputs.complete(&first, &mut repeated)?;
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
        let mut observation = reply(&other);
        inputs.complete(&other, &mut observation)?;
        assert_ne!(
            observation["native_input"]["id"],
            result["native_input"]["id"]
        );
        assert_ne!(inputs.directory(&other.declaration)?, scope);
    }
    Ok(())
}
#[test]
fn changed_bytes_or_forged_worker_results_never_create_an_admitted_receipt() -> Result<()> {
    let (_directory, inputs) = fixture()?;
    let first = put(owner(), b"controller-bytes")?;
    inputs.prepare(&first)?;
    let scope = inputs.directory(&first.declaration)?;
    let key = digest(&first.declaration.path)?;
    for invalid in [
        json!({"path":"metadata/authority.json","sha256":first.declaration.sha256,"size":first.declaration.size}),
        json!({"path":first.declaration.path,"sha256":"d".repeat(64),"size":first.declaration.size}),
        json!({"path":first.declaration.path,"sha256":first.declaration.sha256,"size":first.declaration.size+1}),
        json!({"path":first.declaration.path,"sha256":first.declaration.sha256,"size":first.declaration.size,"native_input":{"id":"forged"}}),
    ] {
        assert!(inputs.complete(&first, &mut invalid.clone()).is_err());
        assert!(!scope.join(format!("{key}.receipt.json")).exists());
    }
    assert!(inputs.prepare(&put(owner(), b"changed-request")?).is_err());
    fs::write(scope.join(format!("{key}.data")), b"modified-content")?;
    assert!(inputs.prepare(&first).is_err());
    assert!(inputs.complete(&first, &mut reply(&first)).is_err());
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
    assert!(inputs.complete(&input, &mut reply(&input)).is_err());
    fs::remove_file(&alias)?;
    inputs.prepare(&input)?;
    inputs.complete(&input, &mut reply(&input))?;
    Ok(())
}
