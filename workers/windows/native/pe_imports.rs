//! Controller build-time inspection of trusted bootstrap artifacts. Object owns
//! PE layouts, descriptor iteration and RVA translation; these are build limits.
use anyhow::{Context, Result, ensure};
use object::{
    LittleEndian as LE,
    read::pe::{ImageNtHeaders, ImageOptionalHeader, PeFile},
};
use std::{collections::BTreeSet, io::Read, mem::size_of, path::Path};

const MAX_IMAGE: usize = 64 * 1024 * 1024;

pub fn imported_libraries(path: &Path, architecture: &str) -> Result<Vec<String>> {
    let file = std::fs::File::open(path)?;
    ensure!(
        file.metadata()?.is_file(),
        "expected a regular bootstrap image"
    );
    let mut data = Vec::new();
    file.take(MAX_IMAGE as u64 + 1).read_to_end(&mut data)?;
    inspect(&data, architecture)
}

fn inspect(data: &[u8], architecture: &str) -> Result<Vec<String>> {
    ensure!(data.len() <= MAX_IMAGE, "bootstrap PE image exceeds 64 MiB");
    let parsed = object::File::parse(data).context("read bootstrap PE through object")?;
    match (architecture, parsed) {
        ("x86", object::File::Pe32(pe)) => libraries(&pe, 0x14c),
        ("x64", object::File::Pe64(pe)) => libraries(&pe, 0x8664),
        _ => anyhow::bail!("expected the requested Windows x86/x64 PE architecture"),
    }
}

fn libraries<NT: ImageNtHeaders>(pe: &PeFile<'_, NT>, machine: u16) -> Result<Vec<String>> {
    let nt = pe.nt_headers();
    ensure!(
        nt.file_header().machine.get(LE).0 == machine
            && pe.dos_header().nt_headers_offset() <= 1024 * 1024
            && (1..=96).contains(&nt.file_header().number_of_sections.get(LE))
            && nt.optional_header().size_of_headers() as usize <= pe.data().len(),
        "invalid bootstrap machine, section count or headers"
    );
    let mut result = BTreeSet::new();
    for (index, descriptor_size) in [
        (
            object::pe::IMAGE_DIRECTORY_ENTRY_IMPORT,
            size_of::<object::pe::ImageImportDescriptor>(),
        ),
        (
            object::pe::IMAGE_DIRECTORY_ENTRY_DELAY_IMPORT,
            size_of::<object::pe::ImageDelayloadDescriptor>(),
        ),
    ] {
        let Some(directory) = pe.data_directories().iter().nth(index) else {
            continue;
        };
        let address = directory.virtual_address.get(LE);
        let length = directory.size.get(LE) as usize;
        if address == 0 && length == 0 {
            continue;
        }
        ensure!(
            address != 0 && (descriptor_size..=65536).contains(&length),
            "invalid PE import directory bound"
        );
        directory.data(pe.data(), &pe.section_table())?;
        // Object follows the loader and ignores the directory's declared size.
        // Keep the bootstrap's stricter bound, including its null descriptor.
        let limit = length / descriptor_size;
        let mut terminated = false;
        if index == object::pe::IMAGE_DIRECTORY_ENTRY_IMPORT {
            let table = pe.import_table()?.context("missing PE import directory")?;
            let mut descriptors = table.descriptors()?;
            for _ in 0..limit {
                let Some(record) = descriptors.next()? else {
                    terminated = true;
                    break;
                };
                result.insert(library_name(pe, record.name.get(LE))?);
            }
        } else {
            let table = pe
                .delay_load_import_table()?
                .context("missing PE delay import directory")?;
            let mut descriptors = table.descriptors()?;
            for _ in 0..limit {
                let Some(record) = descriptors.next()? else {
                    terminated = true;
                    break;
                };
                ensure!(
                    record.attributes.get(LE) == object::pe::IMAGE_DELAYLOAD_RVA_BASED,
                    "unsupported PE delay-import addressing"
                );
                result.insert(library_name(pe, record.dll_name_rva.get(LE))?);
            }
        }
        ensure!(terminated, "unterminated PE import directory");
    }
    ensure!(
        !result.is_empty(),
        "native worker has no inspectable system imports"
    );
    Ok(result.into_iter().collect())
}

fn library_name<NT: ImageNtHeaders>(pe: &PeFile<'_, NT>, address: u32) -> Result<String> {
    let data = pe
        .section_table()
        .pe_data_at(pe.data(), address)
        .context("unmapped PE library name")?;
    let end = data
        .iter()
        .take(256)
        .position(|byte| *byte == 0)
        .context("unbounded PE library name")?;
    let name = std::str::from_utf8(&data[..end])
        .context("invalid PE library name")?
        .to_ascii_lowercase();
    let stem = name
        .strip_suffix(".dll")
        .context("invalid PE library name")?;
    ensure!(
        !stem.is_empty()
            && stem
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')),
        "invalid PE library name"
    );
    Ok(name)
}

#[cfg(test)]
#[path = "pe_imports_tests.rs"]
mod tests;
