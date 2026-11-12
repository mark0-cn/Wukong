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
fn fixture() -> Result<(tempfile::TempDir, Inputs)> {
    let directory = tempfile::tempdir()?;
    let policy = AclPolicy::current_user()?;
    policy.apply(directory.path(), true)?;
    let inputs = Inputs::open(directory.path(), policy)?;
    Ok((directory, inputs))
}
fn prepare(inputs: &Inputs, owner: Owner, method: &str, params: Value) -> Result<Transfer> {
    inputs.prepare_transfer(owner, method, params.as_object().unwrap())
}
fn begin(inputs: &Inputs, owner: Owner, data: &[u8]) -> Result<Transfer> {
    let transfer = prepare(
        inputs,
        owner,
        "file.upload.begin",
        json!({"path":"targets/sample.bin","sha256":hex::encode(Sha256::digest(data)),"size":data.len(),"executable":true}),
    )?;
    let mut result = json!({"transfer_id":transfer.transfer_id,"chunk_bytes":CHUNK});
    inputs.complete_transfer(&transfer, &mut result)?;
    Ok(transfer)
}
fn status(transfer: &Transfer, offset: u64, complete: bool) -> Value {
    let mut result =
        json!({"transfer_id":transfer.transfer_id,"offset":offset,"complete":complete});
    if complete {
        result["artifact"] = json!({"path":transfer.declaration.path,"sha256":transfer.declaration.sha256,"size":transfer.declaration.size});
    }
    result
}
fn chunk(inputs: &Inputs, owner: Owner, id: &str, offset: u64, data: &[u8]) -> Result<Transfer> {
    prepare(
        inputs,
        owner,
        "file.upload.write",
        json!({"transfer_id":id,"offset":offset,"data_base64":STANDARD.encode(data)}),
    )
}

#[test]
fn resumable_native_bytes_reconcile_both_offsets_and_commit_exact_fenced_proof() -> Result<()> {
    let (directory, inputs) = fixture()?;
    let transfer = begin(&inputs, owner(), b"abcdef")?;
    let reference = Reference {
        path: transfer.declaration.path.clone(),
        id: digest(&transfer.declaration)?,
        sha256: transfer.declaration.sha256.clone(),
        size: transfer.declaration.size,
        executable: transfer.declaration.executable,
    };
    assert!(
        inputs
            .acquire(&owner(), &reference, Purpose::Execute)
            .is_err()
    );
    // Generated independently by the real Python TransferService fixture.
    assert_eq!(
        transfer.transfer_id,
        "ec079f90e8f713f84af33c720050e56fc06ed0612efbb33cb37be05feb593331"
    );
    let id = &transfer.transfer_id;
    let first = chunk(&inputs, owner(), id, 0, b"abc")?;
    // The native write persisted but an upstream write was not acknowledged.
    // Status takes the lower offset so the controller resends the missing bytes.
    let check = prepare(
        &inputs,
        owner(),
        "file.upload.status",
        json!({"transfer_id":id}),
    )?;
    let mut lagging = status(&check, 0, false);
    inputs.complete_transfer(&check, &mut lagging)?;
    assert_eq!(lagging["offset"], 0);
    let mut acknowledged = status(&first, 3, false);
    inputs.complete_transfer(&first, &mut acknowledged)?;
    assert_eq!(acknowledged["offset"], 3);
    drop(inputs);
    let inputs = Inputs::open(directory.path(), AclPolicy::current_user()?)?;
    chunk(&inputs, owner(), id, 0, b"abc")?;
    assert!(chunk(&inputs, owner(), id, 0, b"wrong").is_err());
    assert!(chunk(&inputs, owner(), id, 4, b"ef").is_err());
    assert!(
        prepare(
            &inputs,
            owner(),
            "file.upload.commit",
            json!({"transfer_id":id})
        )
        .is_err()
    );
    let last = chunk(&inputs, owner(), id, 3, b"def")?;
    inputs.complete_transfer(&last, &mut status(&last, 6, false))?;
    let committed = prepare(
        &inputs,
        owner(),
        "file.upload.commit",
        json!({"transfer_id":id}),
    )?;
    let mut result = status(&committed, 6, true)["artifact"].clone();
    inputs.complete_transfer(&committed, &mut result)?;
    assert_eq!(result["native_input"]["epoch"], 1);
    {
        let mut source = inputs.acquire(&owner(), &reference, Purpose::Execute)?;
        let mut bytes = Vec::new();
        source.read_to_end(&mut bytes)?;
        assert_eq!(bytes, b"abcdef");
    }
    let repeated = prepare(
        &inputs,
        owner(),
        "file.upload.commit",
        json!({"transfer_id":id}),
    )?;
    let mut repeated_result = status(&repeated, 6, true)["artifact"].clone();
    inputs.complete_transfer(&repeated, &mut repeated_result)?;
    assert_eq!(repeated_result, result);
    let check = prepare(
        &inputs,
        owner(),
        "file.upload.status",
        json!({"transfer_id":id}),
    )?;
    let mut status_result = status(&check, 6, true);
    inputs.complete_transfer(&check, &mut status_result)?;
    assert_eq!(status_result["artifact"], result);
    assert_eq!(status_result["complete"], true);
    assert!(chunk(&inputs, owner(), id, 6, b"!").is_err());
    Ok(())
}

#[test]
fn a_worker_file_from_another_epoch_never_skips_native_controller_bytes() -> Result<()> {
    let (_directory, inputs) = fixture()?;
    let old = begin(&inputs, owner(), b"abcdef")?;
    chunk(&inputs, owner(), &old.transfer_id, 0, b"abcdef")?;
    let commit = prepare(
        &inputs,
        owner(),
        "file.upload.commit",
        json!({"transfer_id":old.transfer_id}),
    )?;
    inputs.complete_transfer(&commit, &mut status(&commit, 6, true)["artifact"].clone())?;
    let next_owner = Owner {
        epoch: 2,
        ..owner()
    };
    // A raw old transfer ID alone is not native ownership in the new fence.
    assert!(
        prepare(
            &inputs,
            next_owner.clone(),
            "file.upload.status",
            json!({"transfer_id":old.transfer_id})
        )
        .is_err()
    );
    let new = begin(&inputs, next_owner.clone(), b"abcdef")?;
    assert_eq!(new.transfer_id, old.transfer_id);
    let mut worker_already_has_file = status(&new, 6, true);
    inputs.complete_transfer(
        &Transfer {
            action: Action::Status,
            ..new
        },
        &mut worker_already_has_file,
    )?;
    assert_eq!(worker_already_has_file["offset"], 0);
    assert_eq!(worker_already_has_file["complete"], false);
    assert!(worker_already_has_file.get("artifact").is_none());
    for (offset, data) in [(0, b"abc".as_slice()), (3, b"def".as_slice())] {
        let write = chunk(&inputs, next_owner.clone(), &old.transfer_id, offset, data)?;
        let mut result = status(&write, 6, true);
        inputs.complete_transfer(&write, &mut result)?;
        assert_eq!(result["offset"], offset + data.len() as u64);
        assert_eq!(result["complete"], false);
    }
    let commit = prepare(
        &inputs,
        next_owner,
        "file.upload.commit",
        json!({"transfer_id":old.transfer_id}),
    )?;
    let mut result = status(&commit, 6, true)["artifact"].clone();
    inputs.complete_transfer(&commit, &mut result)?;
    assert_eq!(result["native_input"]["epoch"], 2);
    Ok(())
}

#[test]
fn empty_uploads_work_and_malformed_worker_offsets_or_hashes_never_admit() -> Result<()> {
    let (_directory, inputs) = fixture()?;
    let empty = begin(&inputs, owner(), b"")?;
    let commit = prepare(
        &inputs,
        owner(),
        "file.upload.commit",
        json!({"transfer_id":empty.transfer_id}),
    )?;
    let mut result = status(&commit, 0, true)["artifact"].clone();
    inputs.complete_transfer(&commit, &mut result)?;
    assert_eq!(result["native_input"]["size"], 0);
    let transfer = prepare(
        &inputs,
        owner(),
        "file.upload.begin",
        json!({"path":"other.bin","sha256":"f".repeat(64),"size":3}),
    )?;
    assert!(
        inputs
            .complete_transfer(
                &transfer,
                &mut json!({"transfer_id":"0".repeat(64),"chunk_bytes":CHUNK})
            )
            .is_err()
    );
    let write = chunk(&inputs, owner(), &transfer.transfer_id, 0, b"abc")?;
    assert!(
        inputs
            .complete_transfer(&write, &mut status(&write, 2, false))
            .is_err()
    );
    assert!(
        prepare(
            &inputs,
            owner(),
            "file.upload.commit",
            json!({"transfer_id":transfer.transfer_id})
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn maximum_chunks_split_correctly_and_size_limits_do_not_allocate_declared_artifacts() -> Result<()>
{
    let (_directory, inputs) = fixture()?;
    let data = vec![b'x'; CHUNK as usize + 1];
    let transfer = begin(&inputs, owner(), &data)?;
    assert!(chunk(&inputs, owner(), &transfer.transfer_id, 0, &data).is_err());
    chunk(
        &inputs,
        owner(),
        &transfer.transfer_id,
        0,
        &data[..CHUNK as usize],
    )?;
    assert!(
        prepare(
            &inputs,
            owner(),
            "file.upload.commit",
            json!({"transfer_id":transfer.transfer_id})
        )
        .is_err()
    );
    chunk(
        &inputs,
        owner(),
        &transfer.transfer_id,
        CHUNK,
        &data[CHUNK as usize..],
    )?;
    let commit = prepare(
        &inputs,
        owner(),
        "file.upload.commit",
        json!({"transfer_id":transfer.transfer_id}),
    )?;
    inputs.complete_transfer(
        &commit,
        &mut status(&commit, data.len() as u64, true)["artifact"].clone(),
    )?;
    let largest = prepare(
        &inputs,
        owner(),
        "file.upload.begin",
        json!({"path":"large.bin","size":MAX_TRANSFER,"sha256":"e".repeat(64)}),
    )?;
    let directory = inputs.directory(&largest.declaration)?;
    let key = digest(&largest.declaration.path)?;
    assert!(!directory.join(format!("{key}.data")).exists());
    assert!(!directory.join(format!("{key}.partial")).exists());
    assert!(
        prepare(
            &inputs,
            owner(),
            "file.upload.begin",
            json!({"path":"too-large.bin","size":MAX_TRANSFER+1,"sha256":"e".repeat(64)})
        )
        .is_err()
    );
    Ok(())
}
