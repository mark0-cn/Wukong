//! Binary metadata belongs in the unprivileged debugger helper. Object reads PE;
//! Microsoft's pinned DbgHelp reads PDB; DbgEng observes live/dump modules.
use anyhow::{Context, Result, ensure};
use object::{
    Object,
    read::pe::{ImageNtHeaders, ImageOptionalHeader},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::Read,
    mem::size_of,
    os::windows::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};
use windows::{
    Win32::System::{
        Diagnostics::Debug::{Extensions::*, IMAGEHLP_MODULEW64, SYMSRV_INDEX_INFOW},
        LibraryLoader::{
            GetProcAddress, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32,
            LoadLibraryExW,
        },
    },
    core::{PCWSTR, s},
};
use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ};

fn held_file(path: &Path, maximum: u64) -> Result<File> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.len() > 0 && metadata.len() <= maximum,
        "binary identity file is not a bounded regular file"
    );
    ensure!(
        metadata.file_attributes() & 0x400 == 0,
        "binary identity file cannot be a reparse point"
    );
    Ok(file)
}

fn pe_metadata<NT: ImageNtHeaders>(
    pe: &object::read::pe::PeFile<'_, NT>,
    architecture: &str,
) -> Result<Value> {
    let nt = pe.nt_headers();
    ensure!(
        (1..=96).contains(
            &nt.file_header()
                .number_of_sections
                .get(object::LittleEndian)
        ),
        "PE section count exceeds inspection bound"
    );
    ensure!(
        nt.optional_header().size_of_headers() as usize <= pe.data().len(),
        "PE headers exceed the file"
    );
    let machine = nt.file_header().machine.get(object::LittleEndian).0;
    ensure!(
        (architecture == "x86" && machine == 0x14c) || (architecture == "x64" && machine == 0x8664),
        "PE machine and optional-header architecture differ"
    );
    let size = nt.optional_header().size_of_image();
    ensure!(size > 0, "PE has no mapped image size");
    // Object owns byte layout and RVA translation; these are application limits
    // and ambiguity policy, not a second PE/CodeView parser.
    if let Some(directory) = pe
        .data_directories()
        .iter()
        .nth(object::pe::IMAGE_DIRECTORY_ENTRY_DEBUG)
    {
        let address = directory.virtual_address.get(object::LittleEndian);
        let length = directory.size.get(object::LittleEndian);
        if address != 0 || length != 0 {
            ensure!(
                address != 0
                    && length != 0
                    && length <= 128 * size_of::<object::pe::ImageDebugDirectory>() as u32,
                "invalid or oversized PE debug directory"
            );
            let data = directory.data(pe.data(), &pe.section_table())?;
            let entries =
                object::pod::slice_from_all_bytes::<object::pe::ImageDebugDirectory>(data)
                    .map_err(|_| anyhow::anyhow!("invalid PE debug directory size"))?;
            let mut count = 0;
            for entry in entries {
                if entry.typ.get(object::LittleEndian) == object::pe::IMAGE_DEBUG_TYPE_CODEVIEW {
                    count += 1;
                    ensure!(
                        (4..=65536).contains(&entry.size_of_data.get(object::LittleEndian)),
                        "CodeView record exceeds inspection bound"
                    );
                }
            }
            ensure!(count <= 1, "ambiguous PE CodeView records");
        }
    }
    let codeview = pe.pdb_info()?.map(|info| {
        let name = String::from_utf8_lossy(info.path());
        json!({"format":"RSDS","guid":uuid::Uuid::from_bytes_le(info.guid()).simple().to_string(),
            "age":info.age(),"pdb_name":name.rsplit(['\\','/']).next().unwrap_or("").chars().take(256).collect::<String>()})
    });
    Ok(
        json!({"machine":machine,"architecture":architecture,"size":size,
        "timestamp":nt.file_header().time_date_stamp.get(object::LittleEndian),
        "checksum":nt.optional_header().check_sum(),"codeview":codeview,
        "source":"object_pe"}),
    )
}

pub(super) fn pe(path: &Path) -> Result<Value> {
    let mut file = held_file(path, 256 * 1024 * 1024)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let parsed = object::File::parse(bytes.as_slice()).context("read PE through object")?;
    let mut result = match parsed {
        object::File::Pe32(ref pe) => pe_metadata(pe, "x86")?,
        object::File::Pe64(ref pe) => pe_metadata(pe, "x64")?,
        _ => anyhow::bail!("Windows debugging requires an x86/x64 PE image"),
    };
    result["file_sha256"] = json!(hex::encode(Sha256::digest(&bytes)));
    result["file_size"] = json!(bytes.len());
    Ok(result)
}

/// Called only before DebugCreate. No DbgHelp function overlaps a DbgEng session.
pub(super) fn pdb(path: &Path, engine_directory: &Path) -> Result<Value> {
    let mut file = held_file(path, 4 * 1024 * 1024 * 1024)?;
    let library_path = engine_directory.join("dbghelp.dll");
    let library_name = super::wide(&library_path.to_string_lossy())?;
    let library = unsafe {
        LoadLibraryExW(
            PCWSTR(library_name.as_ptr()),
            None,
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
    }
    .context("load the configured local DbgHelp library")?;
    // DbgHelp keeps process-wide state. Retain the module for this helper's
    // lifetime, and resolve the SDK entry from this explicit pinned directory.
    let function = unsafe { GetProcAddress(library, s!("SymSrvGetFileIndexInfoW")) }
        .context("configured DbgHelp has no file-index API")?;
    type Index =
        unsafe extern "system" fn(PCWSTR, *mut SYMSRV_INDEX_INFOW, u32) -> windows::core::BOOL;
    let function: Index = unsafe { std::mem::transmute(function) };
    let mut info = SYMSRV_INDEX_INFOW {
        sizeofstruct: size_of::<SYMSRV_INDEX_INFOW>() as u32,
        ..Default::default()
    };
    let name = super::wide(&path.to_string_lossy())?;
    unsafe { function(PCWSTR(name.as_ptr()), &mut info, 0).ok() }
        .context("read local PDB through DbgHelp")?;
    ensure!(
        info.guid.to_u128() != 0,
        "local PDB has no supported GUID identity"
    );
    // This SDK also indexes images and DBG files. Their embedded PDB identity
    // must not let an image renamed to .pdb pass as a symbol file.
    ensure!(
        info.timestamp == 0
            && info.size == 0
            && info.sig == 0
            && info.pdbfile.iter().all(|value| *value == 0)
            && info.dbgfile.iter().all(|value| *value == 0),
        "configured symbol is an image/DBG index rather than a GUID PDB index"
    );
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(
        json!({"guid":format!("{:032x}",info.guid.to_u128()),"age":info.age,
        "file_sha256":hex::encode(digest.finalize()),"file_size":file.metadata()?.len(),
        "source":"pinned_dbghelp_file_index"}),
    )
}

pub(super) fn module_codeview(advanced: &IDebugAdvanced3, base: u64) -> Value {
    let mut info = IMAGEHLP_MODULEW64 {
        SizeOfStruct: size_of::<IMAGEHLP_MODULEW64>() as u32,
        ..Default::default()
    };
    let mut returned = 0;
    let observed = unsafe {
        advanced.GetSymbolInformationWide(
            DEBUG_SYMINFO_IMAGEHLP_MODULEW64,
            base,
            0,
            Some((&mut info as *mut IMAGEHLP_MODULEW64).cast()),
            size_of::<IMAGEHLP_MODULEW64>() as u32,
            Some(&mut returned),
            None,
            None,
        )
    };
    if observed.is_err()
        || returned as usize != size_of::<IMAGEHLP_MODULEW64>()
        || info.PdbSig70.to_u128() == 0
    {
        return Value::Null;
    }
    json!({"format":"RSDS","guid":format!("{:032x}",info.PdbSig70.to_u128()),"age":info.PdbAge})
}
