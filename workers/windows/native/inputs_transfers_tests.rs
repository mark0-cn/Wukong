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
fn fixture() -> Result<(tempfile::TempDir, Inputs)> {
    let directory = tempfile::tempdir()?;
    let policy = AclPolicy::current_user()?;
    policy.apply(directory.path(), true)?;
    let inputs = Inputs::open(directory.path(), policy)?;
    Ok((directory, inputs))
}
fn call(inputs: &Inputs, method: &str, params: Value) -> Result<Value> {
    inputs.call(owner(), method, params.as_object().unwrap())
}
fn begin(inputs: &Inputs, data: &[u8]) -> Result<Value> {
    call(
        inputs,
        "file.upload.begin",
        json!({"path":"targets/sample.bin","size":data.len(),"sha256":hex::encode(Sha256::digest(data)),"executable":true}),
    )
}
fn write(inputs: &Inputs, id: &Value, offset: u64, data: &[u8]) -> Result<Value> {
    call(
        inputs,
        "file.upload.write",
        json!({"transfer_id":id,"offset":offset,"data_base64":STANDARD.encode(data)}),
    )
}

#[test]
fn native_upload_resumes_its_own_durable_offset_and_publishes_exact_owner_proof() -> Result<()> {
    let (root, inputs) = fixture()?;
    let begun = begin(&inputs, b"abcdef")?;
    let id = &begun["transfer_id"];
    assert_eq!(begun["chunk_bytes"], CHUNK);
    assert_eq!(write(&inputs, id, 0, b"abc")?["offset"], 3);
    assert!(write(&inputs, id, 0, b"wrong").is_err());
    assert!(write(&inputs, id, 4, b"ef").is_err());
    assert!(call(&inputs, "file.upload.commit", json!({"transfer_id":id})).is_err());
    drop(inputs);
    let inputs = Inputs::open(root.path(), AclPolicy::current_user()?)?;
    assert_eq!(
        call(&inputs, "file.upload.status", json!({"transfer_id":id}))?["offset"],
        3
    );
    assert_eq!(write(&inputs, id, 0, b"abc")?["offset"], 3);
    assert_eq!(write(&inputs, id, 3, b"def")?["offset"], 6);
    assert_eq!(
        call(&inputs, "file.upload.status", json!({"transfer_id":id}))?["complete"],
        false
    );
    let committed = call(&inputs, "file.upload.commit", json!({"transfer_id":id}))?;
    assert_eq!(committed["native_input"]["epoch"], 1);
    assert_eq!(
        call(&inputs, "file.upload.commit", json!({"transfer_id":id}))?,
        committed
    );
    let status = call(&inputs, "file.upload.status", json!({"transfer_id":id}))?;
    assert_eq!(status["complete"], true);
    assert_eq!(status["artifact"], committed);
    assert_eq!(write(&inputs, id, 3, b"def")?["complete"], true);
    assert_eq!(
        call(
            &inputs,
            "file.get",
            json!({"path":"files/targets/sample.bin"})
        )?["data_base64"],
        STANDARD.encode(b"abcdef")
    );
    let chunk = call(
        &inputs,
        "file.read",
        json!({"path":"files/targets/sample.bin","offset":2,"max_bytes":2}),
    )?;
    assert_eq!(chunk["data_base64"], STANDARD.encode(b"cd"));
    assert_eq!(chunk["size"], 6);
    assert!(!root.path().join("files").exists());
    let mut foreign = owner();
    foreign.epoch += 1;
    assert!(
        inputs
            .call(
                foreign,
                "file.upload.status",
                json!({"transfer_id":id}).as_object().unwrap()
            )
            .is_err()
    );
    Ok(())
}

#[test]
fn incomplete_empty_and_tampered_uploads_never_forge_completion() -> Result<()> {
    let (_root, inputs) = fixture()?;
    let id = begin(&inputs, b"")?["transfer_id"].clone();
    assert_eq!(
        call(&inputs, "file.upload.commit", json!({"transfer_id":id}))?["size"],
        0
    );
    let (_other, inputs) = fixture()?;
    let id = begin(&inputs, b"abc")?["transfer_id"].clone();
    write(&inputs, &id, 0, b"abd")?;
    assert!(call(&inputs, "file.upload.commit", json!({"transfer_id":id})).is_err());
    assert_eq!(
        call(&inputs, "file.upload.status", json!({"transfer_id":id}))?["complete"],
        false
    );
    assert!(
        call(
            &inputs,
            "file.get",
            json!({"path":"files/targets/sample.bin"})
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn upload_and_read_budgets_do_not_allocate_declared_artifacts() -> Result<()> {
    let (_root, inputs) = fixture()?;
    let bytes = vec![b'x'; CHUNK as usize + 1];
    let id = begin(&inputs, &bytes)?["transfer_id"].clone();
    assert!(write(&inputs, &id, 0, &bytes).is_err());
    write(&inputs, &id, 0, &bytes[..CHUNK as usize])?;
    write(&inputs, &id, CHUNK, &bytes[CHUNK as usize..])?;
    call(&inputs, "file.upload.commit", json!({"transfer_id":id}))?;
    assert!(
        call(
            &inputs,
            "file.get",
            json!({"path":"files/targets/sample.bin","max_bytes":1})
        )
        .is_err()
    );
    assert!(
        call(
            &inputs,
            "file.read",
            json!({"path":"files/targets/sample.bin","offset":bytes.len()+1})
        )
        .is_err()
    );
    let big = call(
        &inputs,
        "file.upload.begin",
        json!({"path":"large.bin","size":MAX_TRANSFER,"sha256":"e".repeat(64)}),
    )?;
    assert_eq!(big["offset"], 0);
    assert!(
        call(
            &inputs,
            "file.upload.begin",
            json!({"path":"too-large.bin","size":MAX_TRANSFER+1,"sha256":"e".repeat(64)})
        )
        .is_err()
    );
    Ok(())
}
