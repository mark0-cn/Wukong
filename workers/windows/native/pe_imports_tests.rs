use super::*;

fn put16(data: &mut [u8], offset: usize, value: u16) {
    data[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}
fn put32(data: &mut [u8], offset: usize, value: u32) {
    data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
// Inert bytes, never loaded or executed. Preserve the former Python parser's
// format fixtures while exercising the production object inspection directly.
fn fixture(library: &str, delay: bool, architecture: &str) -> Vec<u8> {
    let mut data = vec![0; 1024];
    data[..2].copy_from_slice(b"MZ");
    put32(&mut data, 60, 128);
    data[128..132].copy_from_slice(b"PE\0\0");
    let (machine, magic, base) = if architecture == "x64" {
        (0x8664, 0x20b, 112)
    } else {
        (0x14c, 0x10b, 96)
    };
    put16(&mut data, 132, machine);
    put16(&mut data, 134, 1);
    put16(&mut data, 148, 240);
    put16(&mut data, 152, magic);
    put32(&mut data, 212, 512);
    put32(&mut data, 152 + base - 4, 16);
    let directory = 152 + base + if delay { 13 } else { 1 } * 8;
    put32(&mut data, directory, 4096);
    put32(&mut data, directory + 4, if delay { 64 } else { 40 });
    for (index, value) in [512, 4096, 512, 512].into_iter().enumerate() {
        put32(&mut data, 400 + index * 4, value);
    }
    put32(&mut data, 512 + if delay { 4 } else { 12 }, 4224);
    if delay {
        put32(&mut data, 512, 1);
    }
    data[640..640 + library.len()].copy_from_slice(library.as_bytes());
    data
}

#[test]
fn object_inspects_normal_and_delay_libraries_with_explicit_architecture() -> Result<()> {
    for architecture in ["x64", "x86"] {
        for delay in [false, true] {
            for library in [
                "KERNEL32.dll",
                "VCRUNTIME140.dll",
                "MSVCP140.dll",
                "ucrtbase.dll",
                "ucrtbased.dll",
                "api-ms-win-crt-runtime-l1-1-0.dll",
            ] {
                let data = fixture(library, delay, architecture);
                assert_eq!(
                    inspect(&data, architecture)?,
                    [library.to_ascii_lowercase()]
                );
                assert!(inspect(&data, if architecture == "x64" { "x86" } else { "x64" }).is_err());
                let dir = tempfile::tempdir()?;
                let path = dir.path().join("inert.exe");
                std::fs::write(&path, &data)?;
                assert_eq!(
                    imported_libraries(&path, architecture)?,
                    [library.to_ascii_lowercase()]
                );
            }
        }
    }
    Ok(())
}

#[test]
fn object_refuses_malformed_imports_and_unsafe_names() {
    for architecture in ["x64", "x86"] {
        let base = if architecture == "x64" { 112 } else { 96 };
        for delay in [false, true] {
            for library in [
                "../outside.dll",
                "kernel32.dll:stream",
                "a.dll\\b.dll",
                "a b.dll",
                ".dll",
                "kernel32.exe",
                "é.dll",
            ] {
                assert!(
                    inspect(&fixture(library, delay, architecture), architecture).is_err(),
                    "{library}"
                );
            }
            let original = fixture("kernel32.dll", delay, architecture);
            let directory = 152 + base + if delay { 13 } else { 1 } * 8;
            for (offset, value) in [
                (512 + if delay { 4 } else { 12 }, 0xdeadbeef),
                (directory, 0),
                (directory + 4, 1),
                (directory + 4, 65537),
                (directory + 4, if delay { 32 } else { 20 }),
            ] {
                let mut data = original.clone();
                put32(&mut data, offset, value);
                assert!(
                    inspect(&data, architecture).is_err(),
                    "{architecture} delay={delay} offset={offset}"
                );
            }
            assert!(inspect(&original[..170], architecture).is_err());
            let mut data = original.clone();
            put16(&mut data, 134, 97);
            assert!(inspect(&data, architecture).is_err());
            let mut data = original;
            data[640..896].fill(b'a');
            assert!(inspect(&data, architecture).is_err());
        }
        let mut data = fixture("kernel32.dll", true, architecture);
        put32(&mut data, 512, 0);
        assert!(
            format!("{:#}", inspect(&data, architecture).unwrap_err())
                .contains("delay-import addressing")
        );
    }
}

#[test]
fn empty_and_oversized_images_never_pass_bootstrap_inspection() {
    assert!(inspect(&[], "x64").is_err());
    assert!(inspect(&vec![0; MAX_IMAGE + 1], "x64").is_err());
    let mut data = fixture("kernel32.dll", false, "x64");
    put32(&mut data, 152 + 112 + 8, 0);
    put32(&mut data, 152 + 112 + 12, 0);
    assert!(inspect(&data, "x64").is_err());
    assert!(inspect(&data, "arm64").is_err());
}
