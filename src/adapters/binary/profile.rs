//! Native format inspection and mitigation evidence.
use super::*;
use object::{Object, ObjectSection, ObjectSegment, ObjectSymbol};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MitigationState {
    Enabled,
    Disabled,
    Partial,
    Unknown,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mitigation {
    pub state: MitigationState,
    pub evidence: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SectionInfo {
    pub name: String,
    pub address: AddressRef,
    pub size: u64,
    pub file_offset: Option<u64>,
    pub file_size: Option<u64>,
    pub kind: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolInfo {
    pub name: String,
    pub address: AddressRef,
    pub size: u64,
    pub kind: String,
    pub undefined: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportInfo {
    pub library: String,
    pub name: String,
    pub weak: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinaryProfile {
    pub path: PathBuf,
    pub sha256: String,
    pub size: u64,
    pub format: String,
    pub architecture: String,
    pub endianness: String,
    pub bits: u8,
    pub kind: String,
    /// Link-time module base (ELF lowest PT_LOAD, PE ImageBase, Mach-O __TEXT).
    pub image_base: u64,
    pub entry: AddressRef,
    pub sections: Vec<SectionInfo>,
    pub symbols: Vec<SymbolInfo>,
    pub imports: Vec<ImportInfo>,
    pub libraries: Vec<String>,
    pub has_debug_symbols: bool,
    pub mitigations: BTreeMap<String, Mitigation>,
    pub warnings: Vec<String>,
}

pub fn inspect(path: impl AsRef<Path>) -> Result<BinaryProfile> {
    let path = path.as_ref();
    let data = read_binary(path)?;
    let file = object::File::parse(&*data).context(
        "unsupported or malformed native object (expected ELF, PE, Mach-O or supported object)",
    )?;
    let hash = sha256_bytes(&data);
    let image_base = match file.format() {
        object::BinaryFormat::Elf | object::BinaryFormat::MachO => file
            .segments()
            .filter(|s| s.file_range().1 > 0)
            .map(|s| s.address())
            .min()
            .unwrap_or(0),
        _ => file.relative_address_base(),
    };
    // object::Object::entry reports Mach-O LC_MAIN as a *file offset*, whereas
    // ELF/PE and Mach-O LC_UNIXTHREAD report virtual addresses. Normalize it.
    let mut entry_va = file.entry();
    let entry_offset = match &file {
        object::File::MachO32(file) => macho_entry_offset(file)?,
        object::File::MachO64(file) => macho_entry_offset(file)?,
        _ => None,
    };
    if let Some(off) = entry_offset {
        let segment = file
            .segments()
            .find(|s| {
                let (start, size) = s.file_range();
                off >= start && off - start < size
            })
            .context("Mach-O LC_MAIN entry offset is outside file-backed segments")?;
        entry_va = segment
            .address()
            .checked_add(off - segment.file_range().0)
            .context("Mach-O entry address overflow")?;
    }
    let addr = |a: u64| AddressRef::module_relative(&hash, a.saturating_sub(image_base));
    let mut warnings = Vec::new();
    let sections = file
        .sections()
        .take(MAX_ITEMS + 1)
        .map(|s| {
            let range = s.file_range();
            SectionInfo {
                name: s.name().unwrap_or("<invalid UTF-8>").into(),
                address: addr(s.address()),
                size: s.size(),
                file_offset: range.map(|r| r.0),
                file_size: range.map(|r| r.1),
                kind: format!("{:?}", s.kind()),
            }
        })
        .collect::<Vec<_>>();
    let mut symbols = file
        .symbols()
        .chain(file.dynamic_symbols())
        .take(MAX_ITEMS + 1)
        .map(|s| SymbolInfo {
            name: s.name().unwrap_or("<invalid UTF-8>").into(),
            address: addr(s.address()),
            size: s.size(),
            kind: format!("{:?}", s.kind()),
            undefined: s.is_undefined(),
        })
        .collect::<Vec<_>>();
    symbols.sort_by(|a, b| {
        (&a.name, a.address.value, a.undefined).cmp(&(&b.name, b.address.value, b.undefined))
    });
    symbols
        .dedup_by(|a, b| a.name == b.name && a.address == b.address && a.undefined == b.undefined);
    let mut imports = Vec::new();
    match file.imports() {
        Ok(items) => {
            for item in items.take(MAX_ITEMS + 1) {
                let item = match item {
                    Ok(item) => item,
                    Err(e) => {
                        warnings.push(format!("import entry parsing failed: {e}"));
                        continue;
                    }
                };
                let name = match item.name() {
                    object::read::NameOrOrdinal::Name(n) => String::from_utf8_lossy(n).into_owned(),
                    object::read::NameOrOrdinal::Ordinal(n) => format!("#{n}"),
                };
                imports.push(ImportInfo {
                    library: String::from_utf8_lossy(item.library()).into_owned(),
                    name,
                    weak: item.is_weak(),
                });
            }
        }
        Err(e) => warnings.push(format!("import parsing failed: {e}")),
    }
    let mut libraries = Vec::new();
    match file.import_libraries() {
        Ok(items) => {
            for item in items.take(MAX_ITEMS) {
                match item {
                    Ok(item) => libraries.push(String::from_utf8_lossy(item.name()).into_owned()),
                    Err(e) => warnings.push(format!("dependency entry parsing failed: {e}")),
                }
            }
        }
        Err(e) => warnings.push(format!("dependency parsing failed: {e}")),
    }
    for (label, n) in [
        ("sections", sections.len()),
        ("symbols", symbols.len()),
        ("imports", imports.len()),
    ] {
        if n > MAX_ITEMS {
            warnings.push(format!("{label} truncated at {MAX_ITEMS} entries"));
        }
    }
    let mitigations = mitigations(&file, &imports, &symbols)?;
    Ok(BinaryProfile {
        path: fs::canonicalize(path)?,
        sha256: hash.clone(),
        size: data.len() as u64,
        format: format!("{:?}", file.format()),
        architecture: format!("{:?}", file.architecture()),
        endianness: format!("{:?}", file.endianness()),
        bits: if file.is_64() { 64 } else { 32 },
        kind: format!("{:?}", file.kind()),
        image_base,
        entry: addr(entry_va),
        sections: sections.into_iter().take(MAX_ITEMS).collect(),
        symbols: symbols.into_iter().take(MAX_ITEMS).collect(),
        imports: imports.into_iter().take(MAX_ITEMS).collect(),
        libraries,
        has_debug_symbols: file.has_debug_symbols(),
        mitigations,
        warnings,
    })
}

fn macho_entry_offset<M: object::read::macho::MachHeader>(
    file: &object::read::macho::MachOFile<'_, M>,
) -> Result<Option<u64>> {
    let mut commands = file.macho_load_commands()?;
    while let Some(command) = commands.next()? {
        if let Some(entry) = command.entry_point()? {
            return Ok(Some(entry.entryoff.get(file.endian())));
        }
    }
    Ok(None)
}

fn elf_mitigations<Elf: object::read::elf::FileHeader>(
    file: &object::read::elf::ElfFile<'_, Elf>,
    set: &mut impl FnMut(&str, MitigationState, String),
) -> Result<()> {
    use MitigationState::*;
    use object::{
        elf,
        read::elf::{Dyn, ProgramHeader},
    };
    let endian = file.endian();
    ensure!(
        file.elf_program_headers().len() <= 65536,
        "too many ELF program headers"
    );
    let mut relro = false;
    let mut bind_now = false;
    let mut interpreter = false;
    let mut pie_flag = false;
    for segment in file.elf_program_headers() {
        let typ = segment.p_type(endian);
        if typ == elf::PT_LOAD || typ == elf::PT_DYNAMIC {
            segment
                .data(endian, file.data())
                .map_err(|_| anyhow::anyhow!("ELF segment file range extends beyond target"))?;
        }
        if typ == elf::PT_GNU_STACK {
            let flags = segment.p_flags(endian);
            set(
                "nx",
                if flags.contains(elf::PF_X) {
                    Disabled
                } else {
                    Enabled
                },
                format!("PT_GNU_STACK p_flags={flags:#x}"),
            );
        }
        relro |= typ == elf::PT_GNU_RELRO;
        interpreter |= typ == elf::PT_INTERP;
        if let Some(dynamic) = segment.dynamic(endian, file.data())? {
            ensure!(dynamic.len() <= 65536, "too many ELF dynamic entries");
            for entry in dynamic {
                let tag = entry.d_tag(endian);
                if tag == elf::DT_NULL {
                    break;
                }
                let value = entry.d_val(endian).into();
                bind_now |= tag == elf::DT_BIND_NOW
                    || (tag == elf::DT_FLAGS
                        && elf::DynamicFlags(value).contains(elf::DF_BIND_NOW))
                    || (tag == elf::DT_FLAGS_1
                        && elf::DynamicFlags1(value).contains(elf::DF_1_NOW));
                pie_flag |=
                    tag == elf::DT_FLAGS_1 && elf::DynamicFlags1(value).contains(elf::DF_1_PIE);
            }
        }
    }
    set(
        "relro",
        if relro && bind_now {
            Enabled
        } else if relro {
            Partial
        } else {
            Disabled
        },
        format!("PT_GNU_RELRO={relro}; immediate binding={bind_now}"),
    );
    let typ = file.elf_header().e_type(endian);
    if typ == elf::ET_EXEC {
        set(
            "pie_aslr",
            Disabled,
            "ELF ET_EXEC has fixed link addresses".into(),
        );
    } else if typ == elf::ET_DYN && (interpreter || pie_flag) {
        set("pie_aslr", Enabled, "ELF ET_DYN executable (PT_INTERP or DF_1_PIE); runtime ASLR still depends on OS policy".into());
    } else if typ == elf::ET_DYN {
        set(
            "pie_aslr",
            Unknown,
            "ELF ET_DYN without executable marker may be a shared library".into(),
        );
    }
    Ok(())
}

fn mitigations(
    file: &object::File<'_>,
    imports: &[ImportInfo],
    symbols: &[SymbolInfo],
) -> Result<BTreeMap<String, Mitigation>> {
    use MitigationState::*;
    use object::read::pe::{ImageNtHeaders, ImageOptionalHeader};
    let mut result = BTreeMap::new();
    for key in [
        "nx",
        "pie_aslr",
        "relro",
        "stack_canary",
        "control_flow_guard",
    ] {
        result.insert(
            key.into(),
            Mitigation {
                state: Unknown,
                evidence: "not established for this format/configuration".into(),
            },
        );
    }
    let mut set = |key: &str, state, evidence: String| {
        result.insert(key.into(), Mitigation { state, evidence });
    };
    if imports
        .iter()
        .any(|i| i.name.contains("stack_chk_fail") || i.name.contains("security_check_cookie"))
        || symbols
            .iter()
            .any(|i| i.name.contains("stack_chk_fail") || i.name.contains("security_check_cookie"))
    {
        set(
            "stack_canary",
            Enabled,
            "stack checking symbol present; this does not establish coverage of every function"
                .into(),
        );
    }
    match file {
        object::File::Elf32(file) => elf_mitigations(file, &mut set)?,
        object::File::Elf64(file) => elf_mitigations(file, &mut set)?,
        _ => {}
    }
    let dll_flags = match file {
        object::File::Pe32(file) => Some(file.nt_headers().optional_header().dll_characteristics()),
        object::File::Pe64(file) => Some(file.nt_headers().optional_header().dll_characteristics()),
        _ => None,
    };
    if let Some(flags) = dll_flags {
        for (key, bit, label) in [
            (
                "nx",
                object::pe::IMAGE_DLLCHARACTERISTICS_NX_COMPAT,
                "NX_COMPAT",
            ),
            (
                "pie_aslr",
                object::pe::IMAGE_DLLCHARACTERISTICS_DYNAMIC_BASE,
                "DYNAMIC_BASE",
            ),
            (
                "control_flow_guard",
                object::pe::IMAGE_DLLCHARACTERISTICS_GUARD_CF,
                "GUARD_CF",
            ),
        ] {
            set(
                key,
                if flags.contains(bit) {
                    Enabled
                } else {
                    Disabled
                },
                format!(
                    "PE DLL characteristics {label}={}; loader policy may differ",
                    flags.contains(bit)
                ),
            );
        }
    }
    if let object::FileFlags::MachO { flags } = file.flags() {
        let pie = flags.contains(object::macho::MH_PIE);
        set(
            "pie_aslr",
            if pie { Enabled } else { Disabled },
            format!("Mach-O MH_PIE={pie}"),
        );
        if flags.contains(object::macho::MH_ALLOW_STACK_EXECUTION) {
            set(
                "nx",
                Disabled,
                "Mach-O MH_ALLOW_STACK_EXECUTION present".into(),
            );
        }
    }
    Ok(result)
}
