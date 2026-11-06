use re_agent::binary::{self, AddressRef, AddressSpace, MitigationState, StringsQuery};
use std::fs;

fn put16(b: &mut [u8], i: usize, n: u16) {
    b[i..i + 2].copy_from_slice(&n.to_le_bytes());
}
fn put32(b: &mut [u8], i: usize, n: u32) {
    b[i..i + 4].copy_from_slice(&n.to_le_bytes());
}
fn put64(b: &mut [u8], i: usize, n: u64) {
    b[i..i + 8].copy_from_slice(&n.to_le_bytes());
}
fn elf_fixture() -> Vec<u8> {
    let mut b = vec![0u8; 0x300];
    b[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    put16(&mut b, 16, 2);
    put16(&mut b, 18, 62);
    put32(&mut b, 20, 1);
    put64(&mut b, 24, 0x400100);
    put64(&mut b, 32, 64);
    put16(&mut b, 52, 64);
    put16(&mut b, 54, 56);
    put16(&mut b, 56, 3);
    put16(&mut b, 58, 64);
    // PT_LOAD, executable read-only segment covering the fixture.
    put32(&mut b, 64, 1);
    put32(&mut b, 68, 5);
    put64(&mut b, 80, 0x400000);
    put64(&mut b, 88, 0x400000);
    put64(&mut b, 96, 0x300);
    put64(&mut b, 104, 0x300);
    put64(&mut b, 112, 0x1000);
    // PT_GNU_STACK RW (NX) and GNU_RELRO without immediate binding (partial).
    put32(&mut b, 120, 0x6474e551);
    put32(&mut b, 124, 6);
    put32(&mut b, 176, 0x6474e552);
    put32(&mut b, 180, 4);
    b[0x200..0x20e].copy_from_slice(b"fixture-string");
    b
}
fn macho_fixture() -> Vec<u8> {
    let mut b = vec![0u8; 512];
    put32(&mut b, 0, 0xfeedfacf);
    put32(&mut b, 4, 0x01000007);
    put32(&mut b, 8, 3);
    put32(&mut b, 12, 2);
    put32(&mut b, 16, 2);
    put32(&mut b, 20, 96);
    put32(&mut b, 24, 0x200000);
    put32(&mut b, 32, 0x19);
    put32(&mut b, 36, 72);
    b[40..46].copy_from_slice(b"__TEXT");
    put64(&mut b, 56, 0x100000000);
    put64(&mut b, 64, 512);
    put64(&mut b, 80, 512);
    put32(&mut b, 88, 7);
    put32(&mut b, 92, 5);
    put32(&mut b, 104, 0x80000028);
    put32(&mut b, 108, 24);
    put64(&mut b, 112, 0x100);
    b
}

#[test]
fn elf_addresses_hash_and_mitigation_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("sample.elf");
    let bytes = elf_fixture();
    fs::write(&target, &bytes).unwrap();
    let p = binary::inspect(&target).unwrap();
    assert_eq!(p.format, "Elf");
    assert_eq!(p.architecture, "X86_64");
    assert_eq!(p.bits, 64);
    assert_eq!(p.image_base, 0x400000);
    assert_eq!(p.entry.value, 0x100);
    assert_eq!(p.entry.space, AddressSpace::ModuleRelative);
    assert_eq!(p.sha256, binary::sha256_file(&target).unwrap());
    assert_eq!(p.mitigations["nx"].state, MitigationState::Enabled);
    assert_eq!(p.mitigations["pie_aslr"].state, MitigationState::Disabled);
    assert_eq!(p.mitigations["relro"].state, MitigationState::Partial);
    assert_eq!(
        p.mitigations["stack_canary"].state,
        MitigationState::Unknown
    );
    // Stable content hashes are independent of the path.
    let second = dir.path().join("renamed.elf");
    fs::write(&second, &bytes).unwrap();
    assert_eq!(p.sha256, binary::inspect(second).unwrap().sha256);
}

#[test]
fn macho_lc_main_file_offset_becomes_module_relative() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("sample.macho");
    fs::write(&target, macho_fixture()).unwrap();
    let p = binary::inspect(target).unwrap();
    assert_eq!(p.format, "MachO");
    assert_eq!(p.image_base, 0x100000000);
    assert_eq!(p.entry.value, 0x100);
    assert_eq!(p.mitigations["pie_aslr"].state, MitigationState::Enabled);
}

#[test]
fn host_executable_is_parsed_with_sections() {
    let p = binary::inspect(std::env::current_exe().unwrap()).unwrap();
    assert!(!p.sections.is_empty());
    assert!(!p.sha256.is_empty());
    assert!(matches!(p.format.as_str(), "Pe" | "Elf" | "MachO"));
    assert!(
        p.sections
            .iter()
            .any(|s| s.file_size.is_some_and(|v| v > 0))
    );
    #[cfg(windows)]
    {
        assert_eq!(p.format, "Pe");
        assert!(!p.imports.is_empty());
        assert_eq!(p.mitigations["nx"].state, MitigationState::Enabled);
    }
}

#[test]
fn invalid_binaries_and_invalid_runtime_addresses_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("bad");
    for bytes in [
        vec![],
        b"not an executable".to_vec(),
        b"\x7fELF\x02".to_vec(),
    ] {
        fs::write(&target, bytes).unwrap();
        assert!(binary::inspect(&target).is_err());
    }
    assert!(AddressRef::runtime("hash", 0x100, "", 0).is_err());
    assert!(AddressRef::runtime("hash", 0x100, "run", 0x200).is_err());
    let a = AddressRef::runtime("hash", 0x300, "run", 0x200).unwrap();
    assert_eq!(a.space, AddressSpace::Runtime);
    assert_eq!(a.runtime_instance_id.as_deref(), Some("run"));
}

#[test]
fn strings_are_bounded_paginated_and_keep_file_offsets() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("strings");
    fs::write(&target, b"\0first-string\0second-string\0third\0").unwrap();
    let q = StringsQuery {
        limit: 1,
        max_length: 5,
        ..Default::default()
    };
    let r = binary::strings(&target, &q).unwrap();
    assert_eq!(r.strings[0].text, "first");
    assert!(r.strings[0].truncated);
    assert_eq!(r.strings[0].address.value, 1);
    assert_eq!(r.strings[0].address.space, AddressSpace::FileOffset);
    let r2 = binary::strings(
        &target,
        &StringsQuery {
            offset: r.next_offset.unwrap(),
            ..q
        },
    )
    .unwrap();
    assert_eq!(r2.strings[0].text, "secon");
    assert!(
        binary::strings(
            &target,
            &StringsQuery {
                limit: 0,
                ..Default::default()
            }
        )
        .is_err()
    );
    assert!(
        binary::strings(
            &target,
            &StringsQuery {
                offset: 999,
                ..Default::default()
            }
        )
        .is_err()
    );
}

#[tokio::test]
async fn missing_ghidra_is_an_error_not_a_fake_analysis() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        binary::GhidraBackend::new(binary::GhidraConfig {
            executable: dir.path().join("missing-headless"),
            cache_dir: dir.path().join("cache"),
            script_dir: None,
            timeout_seconds: 1,
            analysis_timeout_seconds: 1,
            max_cpu: 1,
        })
        .is_err()
    );
}

/// Explicit external-tool test: RE_AGENT_GHIDRA_HEADLESS must point at a real
/// Ghidra installation with its JDK configured. It never substitutes fixtures
/// for decompilation, and validates import, cached queries and project reuse.
#[tokio::test]
#[ignore = "requires installed Ghidra and JDK; run with --ignored"]
async fn real_ghidra_import_decompile_xrefs_and_cache() {
    let dir = tempfile::tempdir().unwrap();
    let backend = binary::GhidraBackend::new(
        binary::GhidraConfig::from_env(dir.path().join("ghidra")).unwrap(),
    )
    .unwrap();
    let target = std::env::current_exe().unwrap();
    let query = binary::GhidraQuery::Functions {
        pattern: None,
        offset: 0,
        limit: 2,
    };
    let first = backend.query(&target, &query).await.unwrap();
    assert!(!first.cached);
    assert!(first.stdout_path.is_file());
    let functions = first.data["functions"].as_array().unwrap();
    assert!(!functions.is_empty());
    let second = backend.query(&target, &query).await.unwrap();
    assert!(second.cached);
    assert_eq!(first.cache_key, second.cache_key);
    let address = functions[0]["address"]["value"].as_u64().unwrap();
    let code = backend
        .query(&target, &binary::GhidraQuery::Decompile { address })
        .await
        .unwrap();
    assert!(!code.data["code"].as_str().unwrap().is_empty());
    assert!(std::path::Path::new(code.data["artifact_path"].as_str().unwrap()).is_file());
    let refs = backend
        .query(
            &target,
            &binary::GhidraQuery::Xrefs {
                address,
                direction: binary::XrefDirection::Both,
                limit: 10,
            },
        )
        .await
        .unwrap();
    assert!(refs.data["references"].is_array());
    assert_eq!(first.target_sha256, code.target_sha256);
    assert_ne!(first.cache_key, code.cache_key);
}
