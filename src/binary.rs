//! Bounded native binary inspection and the Ghidra headless worker protocol.
//!
//! All addresses carry their address space. Static inspection never pretends to
//! know a process's ASLR load address; unsupported mitigation checks stay unknown.
use anyhow::{Context, Result, bail, ensure};
use object::{Object, ObjectSection, ObjectSegment, ObjectSymbol};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

const MAX_BINARY_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ITEMS: usize = 20_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AddressSpace {
    FileOffset,
    ModuleRelative,
    Runtime,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AddressRef {
    pub module_sha256: String,
    pub space: AddressSpace,
    pub value: u64,
    pub runtime_instance_id: Option<String>,
    pub load_base: Option<u64>,
}
impl AddressRef {
    pub fn module_relative(hash: &str, value: u64) -> Self {
        Self {
            module_sha256: hash.into(),
            space: AddressSpace::ModuleRelative,
            value,
            runtime_instance_id: None,
            load_base: None,
        }
    }
    pub fn file_offset(hash: &str, value: u64) -> Self {
        Self {
            space: AddressSpace::FileOffset,
            ..Self::module_relative(hash, value)
        }
    }
    pub fn runtime(hash: &str, value: u64, instance: &str, base: u64) -> Result<Self> {
        if instance.is_empty() || value < base {
            bail!("runtime addresses require an instance and value >= load_base");
        }
        Ok(Self {
            module_sha256: hash.into(),
            space: AddressSpace::Runtime,
            value,
            runtime_instance_id: Some(instance.into()),
            load_base: Some(base),
        })
    }
    pub fn validate(&self) -> Result<()> {
        if self.module_sha256.len() != 64
            || !self.module_sha256.bytes().all(|b| b.is_ascii_hexdigit())
        {
            bail!("address module_sha256 must be a 64-character SHA256");
        }
        match self.space {
            AddressSpace::Runtime
                if self
                    .runtime_instance_id
                    .as_ref()
                    .is_none_or(String::is_empty)
                    || self.load_base.is_none_or(|base| base > self.value) =>
            {
                bail!("runtime address requires an instance and valid load_base")
            }
            AddressSpace::FileOffset | AddressSpace::ModuleRelative
                if self.runtime_instance_id.is_some() || self.load_base.is_some() =>
            {
                bail!("static address cannot contain runtime instance/load base")
            }
            _ => Ok(()),
        }
    }
}

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

pub fn sha256_bytes(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}
pub fn sha256_file(path: impl AsRef<Path>) -> Result<String> {
    let mut file = fs::File::open(path.as_ref())
        .with_context(|| format!("open {}", path.as_ref().display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}
fn read_binary(path: &Path) -> Result<Vec<u8>> {
    let file = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    if !file.metadata()?.is_file() {
        bail!("target must be a regular file");
    }
    if file.metadata()?.len() > MAX_BINARY_BYTES {
        bail!("binary exceeds 512 MiB inspection limit");
    }
    let mut data = Vec::new();
    file.take(MAX_BINARY_BYTES + 1).read_to_end(&mut data)?;
    if data.len() as u64 > MAX_BINARY_BYTES {
        bail!("binary grew beyond inspection limit");
    }
    Ok(data)
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StringsQuery {
    pub min_length: usize,
    pub max_length: usize,
    pub limit: usize,
    pub offset: u64,
    pub contains: Option<String>,
}
impl Default for StringsQuery {
    fn default() -> Self {
        Self {
            min_length: 4,
            max_length: 4096,
            limit: 50,
            offset: 0,
            contains: None,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinaryString {
    pub address: AddressRef,
    pub text: String,
    pub truncated: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StringsResult {
    pub sha256: String,
    pub strings: Vec<BinaryString>,
    pub next_offset: Option<u64>,
    pub encoding: String,
}
pub fn strings(path: impl AsRef<Path>, query: &StringsQuery) -> Result<StringsResult> {
    if query.min_length == 0
        || query.min_length > query.max_length
        || query.max_length > 65536
        || query.limit == 0
        || query.limit > 5000
    {
        bail!("invalid strings bounds (length 1..65536, limit 1..5000)");
    }
    let data = read_binary(path.as_ref())?;
    let hash = sha256_bytes(&data);
    let mut pos = usize::try_from(query.offset).context("offset out of range")?;
    if pos > data.len() {
        bail!("strings offset beyond end of file");
    }
    // Resume only at a run boundary; a caller-supplied offset in a run skips it.
    if pos > 0
        && data
            .get(pos.wrapping_sub(1))
            .is_some_and(|b| (0x20..=0x7e).contains(b))
    {
        while pos < data.len() && (0x20..=0x7e).contains(&data[pos]) {
            pos += 1;
        }
    }
    let mut found = Vec::new();
    while pos < data.len() {
        if !(0x20..=0x7e).contains(&data[pos]) {
            pos += 1;
            continue;
        }
        let start = pos;
        while pos < data.len() && (0x20..=0x7e).contains(&data[pos]) {
            pos += 1;
        }
        if pos - start < query.min_length {
            continue;
        }
        let bytes = &data[start..pos];
        if query.contains.as_ref().is_some_and(|needle| {
            !bytes
                .windows(needle.len().max(1))
                .any(|w| w == needle.as_bytes())
                && !needle.is_empty()
        }) {
            continue;
        }
        let end = pos.min(start.saturating_add(query.max_length));
        found.push(BinaryString {
            address: AddressRef::file_offset(&hash, start as u64),
            text: String::from_utf8_lossy(&data[start..end]).into_owned(),
            truncated: end < pos,
        });
        if found.len() == query.limit {
            break;
        }
    }
    Ok(StringsResult {
        sha256: hash,
        strings: found,
        next_offset: (pos < data.len()).then_some(pos as u64),
        encoding: "ASCII printable 0x20..0x7e".into(),
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GhidraConfig {
    pub executable: PathBuf,
    pub cache_dir: PathBuf,
    pub script_dir: Option<PathBuf>,
    pub timeout_seconds: u64,
    pub analysis_timeout_seconds: u64,
    pub max_cpu: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum GhidraQuery {
    Functions {
        pattern: Option<String>,
        offset: usize,
        limit: usize,
    },
    /// Module-relative offset, not a runtime VA or file offset.
    Decompile { address: u64 },
    Xrefs {
        address: u64,
        direction: XrefDirection,
        limit: usize,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum XrefDirection {
    To,
    From,
    Both,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GhidraResult {
    pub cache_key: String,
    pub target_sha256: String,
    pub tool_version: String,
    pub cached: bool,
    pub data: serde_json::Value,
    pub stdout_path: PathBuf,
    pub stderr_path: PathBuf,
}
pub struct GhidraBackend {
    config: GhidraConfig,
}
impl GhidraBackend {
    pub fn new(config: GhidraConfig) -> Result<Self> {
        if config.timeout_seconds == 0
            || config.analysis_timeout_seconds == 0
            || config.max_cpu == 0
        {
            bail!("Ghidra timeouts and max_cpu must be positive");
        }
        if !config.executable.is_file() {
            bail!(
                "Ghidra executable does not exist: {}",
                config.executable.display()
            );
        }
        Ok(Self { config })
    }
    pub async fn query(
        &self,
        target: impl AsRef<Path>,
        query: &GhidraQuery,
    ) -> Result<GhidraResult> {
        match query {
            GhidraQuery::Functions { limit, .. } | GhidraQuery::Xrefs { limit, .. }
                if *limit == 0 || *limit > 5000 =>
            {
                bail!("Ghidra query limit must be 1..5000")
            }
            _ => {}
        }
        let target = fs::canonicalize(target.as_ref())?;
        let hash = sha256_file(&target)?;
        let executable = fs::canonicalize(&self.config.executable)?;
        let props = executable
            .parent()
            .and_then(Path::parent)
            .map(|p| p.join("Ghidra/application.properties"));
        let properties = props
            .as_ref()
            .and_then(|p| fs::read_to_string(p).ok())
            .unwrap_or_default();
        let version = properties
            .lines()
            .find_map(|l| l.strip_prefix("application.version="))
            .unwrap_or("unknown")
            .trim()
            .to_owned();
        let (script_bytes, script_dir) = if let Some(path) = &self.config.script_dir {
            (
                fs::read(path.join("ReAgentBridge.java"))
                    .context("read configured Ghidra bridge script")?,
                fs::canonicalize(path)?,
            )
        } else {
            let bytes = include_bytes!("../workers/ghidra/ReAgentBridge.java").to_vec();
            // A versioned directory prevents concurrent processes overwriting a running bridge.
            let dir = self
                .config
                .cache_dir
                .join("scripts")
                .join(sha256_bytes(&bytes));
            fs::create_dir_all(&dir)?;
            let script = dir.join("ReAgentBridge.java");
            if !script.is_file() {
                fs::write(&script, &bytes)?;
            }
            (bytes, fs::canonicalize(dir)?)
        };
        let analysis_binding = serde_json::json!({"schema":1,"target_sha256":hash,"tool_version":version,"tool_properties":sha256_bytes(properties.as_bytes()),"launcher":sha256_file(&executable)?,"bridge":sha256_bytes(&script_bytes),"analysis_timeout_seconds":self.config.analysis_timeout_seconds,"max_cpu":self.config.max_cpu});
        let analysis_key = sha256_bytes(&serde_json::to_vec(&analysis_binding)?);
        let binding = serde_json::json!({"analysis":analysis_binding,"query":query});
        let key = sha256_bytes(&serde_json::to_vec(&binding)?);
        let root = self.config.cache_dir.join("results").join(&key);
        fs::create_dir_all(&root)?;
        let root = fs::canonicalize(root)?;
        let cache = root.join("result.json");
        let cached = |root: &Path| -> Option<Result<GhidraResult>> {
            let cache = root.join("result.json");
            if !cache.is_file() {
                return None;
            }
            Some((|| {
                let mut result: GhidraResult = serde_json::from_slice(&fs::read(cache)?)
                    .context("invalid Ghidra cache; remove entry and retry")?;
                if result.target_sha256 != hash || result.cache_key != key {
                    bail!("Ghidra cache identity mismatch");
                }
                result.cached = true;
                Ok(result)
            })())
        };
        if let Some(result) = cached(&root) {
            return result;
        }
        // Keep a single imported program per target/config/tool identity. Queries
        // share this project while a lock guards Ghidra's single-writer database.
        let project = self.config.cache_dir.join("projects").join(&analysis_key);
        fs::create_dir_all(&project)?;
        let project = fs::canonicalize(project)?;
        let lock_path = project.join("worker.lock");
        // A holder keeps the lock for the whole Ghidra run, so waiting must
        // span the configured query budget. Concurrent identical queries are
        // served from the cache the moment the holder publishes its result,
        // which usually ends the wait long before the deadline.
        let deadline =
            tokio::time::Instant::now() + Duration::from_secs(self.config.timeout_seconds);
        let lock = loop {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lock_path)
            {
                Ok(file) => {
                    let owner = serde_json::json!({
                        "owner_pid": std::process::id(),
                        "created_at_unix": std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0),
                    });
                    if let Err(error) = fs::write(&lock_path, serde_json::to_vec(&owner)?) {
                        drop(file);
                        let _ = fs::remove_file(&lock_path);
                        return Err(error.into());
                    }
                    break ProjectLock {
                        path: lock_path.clone(),
                        file: Some(file),
                        owner_pid: std::process::id(),
                    };
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if let Some(result) = cached(&root) {
                        return result;
                    }
                    if tokio::time::Instant::now() >= deadline {
                        bail!(
                            "Ghidra project is busy or has a stale worker.lock; inspect worker state before removing the lock"
                        );
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Err(e) => return Err(e.into()),
            }
        };
        // The lock can be won between the last poll and the successful open;
        // recheck once so a just-published result is not recomputed.
        if let Some(result) = cached(&root) {
            return result;
        }
        let _lock = lock;
        let ready = project.join("analysis-complete.json");
        let snapshot = project.join("target.bin");
        if !ready.exists() {
            fs::copy(&target, &snapshot)?;
            if sha256_file(&snapshot)? != hash {
                bail!("target changed while creating Ghidra snapshot");
            }
        }
        let work = root.join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&work)?;
        let request = work.join("request.json");
        let output = work.join("response.json");
        fs::write(
            &request,
            serde_json::to_vec(&serde_json::json!({"query":query,"target_sha256":hash}))?,
        )?;
        let stdout_path = work.join("stdout.log");
        let stderr_path = work.join("stderr.log");
        fs::write(
            work.join("binding.json"),
            serde_json::to_vec_pretty(&binding)?,
        )?;
        let mut cmd = tokio::process::Command::new(&executable);
        cmd.arg(&project).arg("analysis");
        if ready.is_file() {
            cmd.arg("-process").arg("target.bin").arg("-noanalysis");
        } else {
            cmd.arg("-import").arg(&snapshot).arg("-overwrite");
        }
        cmd.arg("-analysisTimeoutPerFile")
            .arg(self.config.analysis_timeout_seconds.to_string())
            .arg("-max-cpu")
            .arg(self.config.max_cpu.to_string())
            .arg("-scriptPath")
            .arg(&script_dir)
            .arg("-postScript")
            .arg("ReAgentBridge.java")
            .arg(&request)
            .arg(&output)
            .stdin(Stdio::null())
            .stdout(fs::File::create(&stdout_path)?)
            .stderr(fs::File::create(&stderr_path)?)
            .kill_on_drop(false);
        #[cfg(windows)]
        {
            cmd.creation_flags(0x08000000);
        }
        #[cfg(unix)]
        {
            cmd.process_group(0);
        }
        let child = cmd
            .spawn()
            .with_context(|| format!("launch Ghidra {}", executable.display()))?;
        let mut killer = TreeKill { child: Some(child) };
        let status = match tokio::time::timeout(
            Duration::from_secs(self.config.timeout_seconds),
            killer
                .child
                .as_mut()
                .context("Ghidra child missing")?
                .wait(),
        )
        .await
        {
            Ok(status) => status?,
            Err(_) => {
                killer.terminate().await;
                bail!(
                    "Ghidra timed out after {} seconds; logs: {} and {}",
                    self.config.timeout_seconds,
                    stdout_path.display(),
                    stderr_path.display()
                );
            }
        };
        if !status.success() {
            bail!(
                "Ghidra exited {status}; logs: {} and {}",
                stdout_path.display(),
                stderr_path.display()
            );
        }
        let response = fs::read(&output).with_context(|| {
            format!(
                "Ghidra produced no response; inspect {}",
                stdout_path.display()
            )
        })?;
        if response.len() > 16 * 1024 * 1024 {
            bail!("Ghidra response exceeds 16 MiB");
        }
        let data: serde_json::Value =
            serde_json::from_slice(&response).context("Ghidra response is not JSON")?;
        if data.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            bail!(
                "Ghidra bridge failed: {data}; logs: {}",
                stdout_path.display()
            );
        }
        if data.get("target_sha256").and_then(|v| v.as_str()) != Some(&hash) {
            bail!("Ghidra response target identity mismatch");
        }
        if sha256_file(&target)? != hash {
            bail!("target changed during Ghidra analysis; result rejected");
        }
        fs::write(ready, serde_json::to_vec_pretty(&analysis_binding)?)?;
        let result = GhidraResult {
            cache_key: key,
            target_sha256: hash,
            tool_version: version,
            cached: false,
            data,
            stdout_path,
            stderr_path,
        };
        let staged = work.join("result.json");
        fs::write(&staged, serde_json::to_vec_pretty(&result)?)?;
        // Same-key concurrent queries can both complete; either successful result is valid.
        if !cache.exists()
            && let Err(e) = fs::rename(&staged, &cache)
            && !cache.exists()
        {
            return Err(e.into());
        }
        Ok(result)
    }
}

struct ProjectLock {
    path: PathBuf,
    file: Option<fs::File>,
    owner_pid: u32,
}
impl Drop for ProjectLock {
    fn drop(&mut self) {
        drop(self.file.take());
        // Only remove a lock this process still owns. A stale foreign lock
        // must be inspected by an operator, not silently deleted under a
        // live successor.
        let owned = fs::read_to_string(&self.path)
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .and_then(|value| value["owner_pid"].as_u64())
            .is_some_and(|pid| pid == u64::from(self.owner_pid));
        if owned {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Owns the Ghidra child and terminates its whole process tree on drop or
/// timeout. analyzeHeadless launchers spawn the real analyzer as a
/// grandchild (a shell or cmd shim in between), which kill_on_drop alone
/// would leave running after a cancellation.
struct TreeKill {
    child: Option<tokio::process::Child>,
}
impl TreeKill {
    async fn terminate(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        #[cfg(windows)]
        if let Some(pid) = child.id() {
            let _ = tokio::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .creation_flags(0x08000000)
                .output()
                .await;
        }
        #[cfg(unix)]
        if let Some(pid) = child.id() {
            let _ = tokio::process::Command::new("kill")
                .args(["-KILL", "--", &format!("-{pid}")])
                .output()
                .await;
        }
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
}
impl Drop for TreeKill {
    fn drop(&mut self) {
        if let Some(child) = self.child.take() {
            let mut killer = TreeKill { child: Some(child) };
            tokio::spawn(async move { killer.terminate().await });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn analysis_key(backend: &GhidraBackend, target: &Path) -> Result<String> {
        let hash = sha256_file(target)?;
        let executable = fs::canonicalize(&backend.config.executable)?;
        let binding = json!({"schema":1,"target_sha256":hash,"tool_version":"unknown","tool_properties":sha256_bytes(b""),"launcher":sha256_file(&executable)?,"bridge":sha256_bytes(include_bytes!("../workers/ghidra/ReAgentBridge.java")),"analysis_timeout_seconds":backend.config.analysis_timeout_seconds,"max_cpu":backend.config.max_cpu});
        Ok(sha256_bytes(&serde_json::to_vec(&binding)?))
    }

    fn query_key(backend: &GhidraBackend, target: &Path, query: &GhidraQuery) -> Result<String> {
        let hash = sha256_file(target)?;
        let executable = fs::canonicalize(&backend.config.executable)?;
        let analysis_binding = json!({"schema":1,"target_sha256":hash,"tool_version":"unknown","tool_properties":sha256_bytes(b""),"launcher":sha256_file(&executable)?,"bridge":sha256_bytes(include_bytes!("../workers/ghidra/ReAgentBridge.java")),"analysis_timeout_seconds":backend.config.analysis_timeout_seconds,"max_cpu":backend.config.max_cpu});
        Ok(sha256_bytes(&serde_json::to_vec(
            &json!({"analysis":analysis_binding,"query":query}),
        )?))
    }

    #[tokio::test]
    async fn lock_wait_polls_the_cache_and_never_reruns_the_holder_work() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let executable = dir.path().join("fake-ghidra");
        fs::write(&executable, b"exit 1")?;
        let target = dir.path().join("sample.bin");
        fs::write(&target, b"target bytes")?;
        let backend = GhidraBackend::new(GhidraConfig {
            executable,
            cache_dir: dir.path().join("cache"),
            script_dir: None,
            timeout_seconds: 5,
            analysis_timeout_seconds: 2,
            max_cpu: 1,
        })
        .unwrap();
        let query = GhidraQuery::Functions {
            pattern: None,
            offset: 0,
            limit: 5,
        };
        let project = backend
            .config
            .cache_dir
            .join("projects")
            .join(analysis_key(&backend, &target)?);
        fs::create_dir_all(&project)?;
        // A concurrent holder already owns the project lock.
        let lock_path = project.join("worker.lock");
        fs::File::create(&lock_path)?;
        let root = backend
            .config
            .cache_dir
            .join("results")
            .join(query_key(&backend, &target, &query)?);
        fs::create_dir_all(&root)?;
        // The holder publishes a valid cache result after a short delay; the
        // waiting query must serve it instead of timing out or invoking the
        // fake Ghidra executable.
        let result = GhidraResult {
            cache_key: query_key(&backend, &target, &query)?,
            target_sha256: sha256_file(&target)?,
            tool_version: "unknown".into(),
            cached: false,
            data: json!({"functions":[{"name":"main"}]}),
            stdout_path: root.join("stdout.log"),
            stderr_path: root.join("stderr.log"),
        };
        let holder_root = root.clone();
        let holder_lock = lock_path.clone();
        let holder = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            let staged = holder_root.join("result.json.tmp");
            fs::write(&staged, serde_json::to_vec_pretty(&result).unwrap()).unwrap();
            fs::rename(&staged, holder_root.join("result.json")).unwrap();
            let _ = fs::remove_file(holder_lock);
        });
        let started = std::time::Instant::now();
        let served = backend.query(&target, &query).await?;
        assert!(served.cached);
        assert_eq!(served.data["functions"][0]["name"], "main");
        assert!(
            started.elapsed() >= Duration::from_millis(200),
            "the query should have waited for the holder"
        );
        assert!(
            !root.join("response.json").exists(),
            "the fake Ghidra executable must not have been invoked"
        );
        holder.await.unwrap();
        Ok(())
    }

    #[test]
    fn project_lock_drop_removes_only_its_own_lock() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("worker.lock");
        let lock = ProjectLock {
            path: path.clone(),
            file: None,
            owner_pid: std::process::id(),
        };
        // Simulated acquisition: the file records our pid.
        fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({"owner_pid": std::process::id()}))?,
        )?;
        drop(lock);
        assert!(!path.exists(), "an owned lock must be removed on drop");
        // A lock now owned by someone else must survive our drop.
        fs::write(&path, br#"{"owner_pid": 1}"#)?;
        let foreign = ProjectLock {
            path: path.clone(),
            file: None,
            owner_pid: std::process::id(),
        };
        drop(foreign);
        assert!(
            path.exists(),
            "a foreign-owned lock must not be removed on drop"
        );
        fs::remove_file(&path)?;
        Ok(())
    }
}
