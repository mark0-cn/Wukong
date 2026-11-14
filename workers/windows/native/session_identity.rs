//! Correspondence of admitted bytes and the external engine's module records.
//! Object and pdb own format parsing. Metadata correspondence is not a proof
//! that two files have equal contents; the native references retain that hash.
use crate::resource_input::{AdmittedInput, Reference};
use anyhow::{Context, Result, ensure};
use minidump::{Minidump, MinidumpModuleList, MinidumpSystemInfo, Module as _};
use object::{
    LittleEndian as LE, Object,
    read::pe::{ImageNtHeaders, ImageOptionalHeader, PeFile},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, Read, Seek};
use uuid::Uuid;

const MAX_IMAGE: u64 = 64 * 1024 * 1024;
const MAX_PDB_VIEW: usize = 8 * 1024 * 1024;
const PDB_READ_BUDGET: usize = 32 * 1024 * 1024;
pub(crate) const MAX_DUMP: u64 = 256 * 1024 * 1024;

#[derive(Debug, Serialize)]
pub(crate) struct Identity {
    image: Reference,
    pe: PeIdentity,
    pdb: Option<Symbols>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dump: Option<DumpIdentity>,
}
#[derive(Debug, Serialize)]
struct DumpIdentity {
    file: Reference,
    module_base: u64,
    modules: usize,
}
#[derive(Debug, Serialize)]
struct PeIdentity {
    machine: u16,
    timestamp: u32,
    size: u32,
    checksum: u32,
    codeview: Option<CodeView>,
}
#[derive(Debug, Serialize)]
struct CodeView {
    guid: String,
    age: u32,
    name: String,
}
#[derive(Debug, Serialize)]
struct Symbols {
    file: Reference,
    guid: String,
    information_age: u32,
    linker_age: Option<u32>,
    machine: u16,
}

/// Bound the library's view allocation before delegating all reads to its
/// standard Read+Seek adapter. No PDB header or stream layout is decoded here.
#[derive(Debug)]
struct PdbSource<'a> {
    input: &'a mut AdmittedInput,
    remaining: usize,
}
impl<'s, 'a: 's> pdb::Source<'s> for PdbSource<'a> {
    fn view(&mut self, slices: &[pdb::SourceSlice]) -> io::Result<Box<dyn pdb::SourceView<'s>>> {
        let mut length = 0usize;
        for slice in slices {
            let end = slice.offset.checked_add(slice.size as u64);
            length = length.checked_add(slice.size).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "PDB view length overflow")
            })?;
            if end.is_none_or(|end| end > self.input.reference().size)
                || length > MAX_PDB_VIEW
                || length > self.remaining
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "PDB read exceeds admitted bounds",
                ));
            }
        }
        self.remaining -= length;
        pdb::Source::view(&mut self.input, slices)
    }
}

fn basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}
fn pe(data: &[u8]) -> Result<PeIdentity> {
    let image = object::File::parse(data).context("read debugger PE through object")?;
    match image {
        object::File::Pe32(image) => pe_headers(&image, object::pe::IMAGE_FILE_MACHINE_I386.0),
        object::File::Pe64(image) => pe_headers(&image, object::pe::IMAGE_FILE_MACHINE_AMD64.0),
        _ => anyhow::bail!("native debugger requires an x86 or x64 PE image"),
    }
}
fn pe_headers<NT: ImageNtHeaders>(image: &PeFile<'_, NT>, machine: u16) -> Result<PeIdentity> {
    let headers = image.nt_headers();
    let file = headers.file_header();
    let optional = headers.optional_header();
    ensure!(
        file.machine.get(LE).0 == machine && optional.size_of_image() > 0,
        "debugger PE machine or mapped size is unsupported"
    );
    let codeview = image
        .pdb_info()?
        .map(|info| -> Result<CodeView> {
            let name = info
                .path()
                .rsplit(|byte| matches!(byte, b'/' | b'\\'))
                .next()
                .unwrap_or_default();
            let name = std::str::from_utf8(name).context("PE CodeView basename is not UTF-8")?;
            // Never follow the build machine's embedded path. Only an admitted,
            // adjacent portable basename is a candidate for the local symbol path.
            wukong_worker_package::validate_path(name)?;
            ensure!(
                name.len() <= 200 && info.age() > 0,
                "invalid PE CodeView name or age"
            );
            Ok(CodeView {
                guid: Uuid::from_bytes_le(info.guid())
                    .simple()
                    .to_string()
                    .to_ascii_uppercase(),
                age: info.age(),
                name: name.into(),
            })
        })
        .transpose()?;
    Ok(PeIdentity {
        machine,
        timestamp: file.time_date_stamp.get(LE),
        size: optional.size_of_image(),
        checksum: optional.check_sum(),
        codeview,
    })
}
fn symbols(input: &mut AdmittedInput) -> Result<Symbols> {
    let file = input.reference().clone();
    let mut database = pdb::PDB::open(PdbSource {
        input,
        remaining: PDB_READ_BUDGET,
    })?;
    let information = database.pdb_information()?;
    let debug = database.debug_information()?;
    Ok(Symbols {
        file,
        guid: information.guid.simple().to_string().to_ascii_uppercase(),
        information_age: information.age,
        linker_age: debug.age(),
        machine: debug.machine_type()? as u16,
    })
}
fn matches_symbols(image: &PeIdentity, symbols: &Symbols) -> Result<()> {
    let expected = image.codeview.as_ref().context("PE has no RSDS identity")?;
    ensure!(
        symbols.guid == expected.guid
            && symbols.machine == image.machine
            && symbols.information_age >= expected.age
            && symbols.linker_age.is_none_or(|age| age == expected.age),
        "admitted PDB GUID, linker age or machine differs from the target PE"
    );
    Ok(())
}

impl Identity {
    pub(crate) fn admit(sources: &mut [AdmittedInput]) -> Result<Self> {
        let target = sources.first_mut().context("debugger image is absent")?;
        ensure!(
            target.reference().size <= MAX_IMAGE,
            "debugger PE exceeds 64 MiB inspection bound"
        );
        let image = target.reference().clone();
        let (directory, _) = image
            .path
            .rsplit_once('/')
            .context("PE logical parent missing")?;
        ensure!(
            !directory.contains([';', '*']),
            "debugger symbol directory has symbol-path separators"
        );
        let pe = {
            let mut bytes = Vec::new();
            (&mut *target).take(MAX_IMAGE + 1).read_to_end(&mut bytes)?;
            ensure!(
                bytes.len() as u64 == image.size,
                "admitted PE length differs"
            );
            pe(&bytes)?
        };
        let mut selected = None;
        if let Some(codeview) = &pe.codeview {
            let candidate = format!("{directory}/{}", codeview.name);
            if let Some(input) = sources
                .iter_mut()
                .skip(1)
                .find(|input| input.reference().path.eq_ignore_ascii_case(&candidate))
            {
                let pdb = symbols(input).context("read admitted PDB through pdb")?;
                matches_symbols(&pe, &pdb)?;
                selected = Some(pdb);
            }
        }
        // Staging reuses these exact held handles, beginning at byte zero.
        for source in sources {
            source.rewind()?;
        }
        Ok(Self {
            image,
            pe,
            pdb: selected,
            dump: None,
        })
    }
    pub(crate) fn admit_dump(&mut self, input: &mut AdmittedInput) -> Result<()> {
        ensure!(
            !input.reference().executable && input.reference().size <= MAX_DUMP,
            "dump must be non-executable and at most 256 MiB"
        );
        let mut bytes = Vec::new();
        (&mut *input).take(MAX_DUMP + 1).read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 == input.reference().size,
            "admitted dump length differs"
        );
        self.verify_dump(&bytes, input.reference())?;
        input.rewind()?;
        Ok(())
    }
    fn verify_dump(&mut self, bytes: &[u8], file: &Reference) -> Result<()> {
        let dump = Minidump::read(bytes).context("read dump through minidump")?;
        ensure!(
            dump.header.stream_count <= 128,
            "dump stream count exceeds inspection bound"
        );
        for stream in dump.all_streams() {
            dump.get_raw_stream(stream.stream_type)?;
        }
        let system = dump.get_stream::<MinidumpSystemInfo>()?;
        let machine = match system.cpu {
            minidump::system_info::Cpu::X86 => 0x014c,
            minidump::system_info::Cpu::X86_64 => 0x8664,
            _ => anyhow::bail!("dump requires x86 or x64 system information"),
        };
        ensure!(
            system.os == minidump::system_info::Os::Windows && machine == self.pe.machine,
            "dump OS or machine differs from the admitted Windows PE"
        );
        let modules = dump.get_stream::<MinidumpModuleList>()?;
        let count = modules.iter().count();
        ensure!(
            (1..=2000).contains(&count),
            "dump module count exceeds inspection bound"
        );
        let mut selected = modules.iter().filter(|module| {
            basename(&module.name).eq_ignore_ascii_case(basename(&self.image.path))
        });
        let module = selected
            .next()
            .context("admitted image is absent from dump")?;
        ensure!(
            selected.next().is_none(),
            "dump image correspondence is ambiguous"
        );
        ensure!(
            module.raw.time_date_stamp == self.pe.timestamp
                && module.raw.size_of_image == self.pe.size
                && module.raw.checksum == self.pe.checksum,
            "dump module differs from admitted PE metadata"
        );
        ensure!(
            module.raw.base_of_image > 0
                && module
                    .raw
                    .base_of_image
                    .checked_add(u64::from(module.raw.size_of_image))
                    .is_some_and(|end| end <= if machine == 0x14c { 1 << 32 } else { 1 << 47 }),
            "dump target lies outside user address space"
        );
        match (&self.pe.codeview, &module.codeview_info) {
            (Some(expected), Some(minidump::CodeView::Pdb70(_))) => {
                let observed = module
                    .debug_identifier()
                    .context("dump RSDS identity is missing")?;
                ensure!(
                    observed.uuid().simple().to_string().to_ascii_uppercase() == expected.guid
                        && observed.appendix() == expected.age,
                    "dump CodeView GUID or age differs from admitted PE"
                );
            }
            (None, None) => {}
            _ => anyhow::bail!("dump CodeView kind differs from admitted PE"),
        }
        self.dump = Some(DumpIdentity {
            file: file.clone(),
            module_base: module.raw.base_of_image,
            modules: count,
        });
        Ok(())
    }
    pub(crate) fn has_symbols(&self) -> bool {
        self.pdb.is_some()
    }

    pub(crate) fn verify_module(&self, value: &Value) -> Result<()> {
        let module: Module = serde_json::from_value(value.clone())?;
        if let Some(dump) = &self.dump {
            ensure!(
                value["start"] == format!("{:#018x}", dump.module_base),
                "external debugger module base differs from admitted dump"
            );
        }
        ensure!(
            basename(&module.image_name).eq_ignore_ascii_case(basename(&self.image.path))
                && module.timestamp == self.pe.timestamp
                && module.size == u64::from(self.pe.size)
                && module.checksum == self.pe.checksum
                && module.user_mode
                && !module.unloaded,
            "external debugger module differs from admitted target PE"
        );
        if self.pdb.is_some() {
            ensure!(
                module.symbols == "pdb" && module.pdb.is_some(),
                "admitted local PDB was not loaded"
            );
        }
        ensure!(
            (module.symbols == "pdb") == module.pdb.is_some(),
            "MCP PDB state lacks its identity"
        );
        if let Some(observed) = module.pdb {
            let expected = self
                .pe
                .codeview
                .as_ref()
                .context("engine PDB lacks admitted PE identity")?;
            ensure!(
                module.symbols == "pdb"
                    && !observed.unmatched
                    && observed.guid == expected.guid
                    && observed.age >= expected.age
                    && self
                        .pdb
                        .as_ref()
                        .is_none_or(|pdb| observed.age <= pdb.information_age),
                "external debugger PDB identity differs or is unmatched"
            );
        }
        Ok(())
    }
    pub(crate) fn observed<'a>(&self, listing: &'a Value) -> Result<&'a Value> {
        let rows = listing["modules"]
            .as_array()
            .context("MCP module records missing")?;
        ensure!(
            listing["matched"].as_u64() == Some(rows.len() as u64)
                && listing["loaded"].as_u64() == Some(rows.len() as u64),
            "MCP module inventory is truncated"
        );
        let mut found = rows.iter().filter(|row| {
            row["image_name"]
                .as_str()
                .is_some_and(|name| basename(name).eq_ignore_ascii_case(basename(&self.image.path)))
        });
        let module = found
            .next()
            .context("admitted image is absent from MCP module records")?;
        ensure!(
            found.next().is_none(),
            "MCP image correspondence is ambiguous"
        );
        self.verify_module(module)?;
        Ok(module)
    }
    /// Only the admitted main image has an authenticated content identity here.
    /// A path reported for another module never grants permission to open it.
    pub(crate) fn normalize(&self, address: u64, listing: &Value) -> Result<Option<Value>> {
        let module = self.observed(listing)?;
        let base = crate::session_crash::address(&module["start"])?;
        let end = base
            .checked_add(u64::from(self.pe.size))
            .context("module address overflow")?;
        ensure!(
            base > 0
                && end
                    <= if self.pe.machine == 0x14c {
                        1 << 32
                    } else {
                        1 << 47
                    },
            "observed image lies outside user address space"
        );
        Ok((base..end).contains(&address).then(|| {
            serde_json::json!({
            "space":"image_rva","value":address-base,"module_sha256":self.image.sha256,
            "path":self.image.path,"content_source":"admitted_file"})
        }))
    }
}

#[derive(Deserialize)]
struct Module {
    image_name: String,
    timestamp: u32,
    size: u64,
    checksum: u32,
    user_mode: bool,
    #[serde(default)]
    unloaded: bool,
    symbols: String,
    pdb: Option<ModulePdb>,
}
#[derive(Deserialize)]
struct ModulePdb {
    guid: String,
    age: u32,
    #[serde(default)]
    unmatched: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use std::{fs, io::Write, path::Path};

    fn admitted(path: &Path, logical: &str, executable: bool) -> Result<AdmittedInput> {
        let data = fs::read(path)?;
        Ok(AdmittedInput::new(
            fs::File::open(path)?,
            Reference {
                path: logical.into(),
                id: "a".repeat(64),
                sha256: hex::encode(Sha256::digest(&data)),
                size: data.len() as u64,
                executable,
            },
        ))
    }
    fn fixture() -> Result<Vec<AdmittedInput>> {
        // Cargo's own MSVC-linked test artifact is trusted and never executed
        // by this test. Its real PDB exercises library stream/GUID decoding.
        let image = std::env::current_exe()?;
        let pdb = image.with_extension("pdb");
        Ok(vec![
            admitted(
                &image,
                &format!("files/{}", image.file_name().unwrap().to_str().unwrap()),
                true,
            )?,
            admitted(
                &pdb,
                &format!("files/{}", pdb.file_name().unwrap().to_str().unwrap()),
                false,
            )?,
        ])
    }
    #[test]
    #[ignore = "trusted child used by the SDK minidump test"]
    fn dump_child_fixture() -> Result<()> {
        use std::io::Write;
        println!("wukong-dump-child-ready");
        std::io::stdout().flush()?;
        let mut byte = [0];
        std::io::stdin().read_exact(&mut byte)?;
        Ok(())
    }
    #[test]
    fn sdk_minidump_binds_image_symbols_and_rejects_mismatches() -> Result<()> {
        use std::{
            io::{BufRead, BufReader},
            os::windows::io::AsRawHandle,
            process::{Child, Command, Stdio},
        };
        use windows_sys::Win32::System::Diagnostics::Debug::{MiniDumpNormal, MiniDumpWriteDump};
        struct OwnedChild(Child);
        impl Drop for OwnedChild {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut child = OwnedChild(
            Command::new(std::env::current_exe()?)
                .args([
                    "--ignored",
                    "--exact",
                    "session_identity::tests::dump_child_fixture",
                    "--nocapture",
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()?,
        );
        let mut output = BufReader::new(child.0.stdout.take().unwrap());
        let mut ready = false;
        for _ in 0..8 {
            let mut line = String::new();
            if output.read_line(&mut line)? == 0 {
                break;
            }
            if line.trim() == "wukong-dump-child-ready" {
                ready = true;
                break;
            }
        }
        ensure!(ready, "trusted dump fixture did not start");
        let file = tempfile::NamedTempFile::new()?;
        crate::security::check(unsafe {
            MiniDumpWriteDump(
                child.0.as_raw_handle(),
                child.0.id(),
                file.as_file().as_raw_handle(),
                MiniDumpNormal,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
            )
        })?;
        child.0.stdin.take().unwrap().write_all(&[1])?;
        ensure!(child.0.wait()?.success(), "trusted dump fixture failed");
        let mut identity = Identity::admit(&mut fixture()?)?;
        let mut input = admitted(file.path(), "files/fixture.dmp", false)?;
        identity.admit_dump(&mut input)?;
        assert_eq!(input.stream_position()?, 0);
        let bound = identity.dump.as_ref().unwrap();
        assert_eq!(&bound.file, input.reference());
        assert!(bound.modules > 0);
        let mut observed = module(&identity);
        observed["start"] = json!(format!("{:#018x}", bound.module_base));
        identity.verify_module(&observed)?;
        observed["start"] = json!("0x0000000000000000");
        assert!(identity.verify_module(&observed).is_err());
        let bytes = fs::read(file.path())?;
        let reference = input.reference().clone();
        assert!(identity.verify_dump(&bytes[..4], &reference).is_err());
        let mut truncated = tempfile::NamedTempFile::new()?;
        truncated.write_all(&bytes[..bytes.len() / 2])?;
        assert!(
            identity
                .admit_dump(&mut AdmittedInput::new(
                    fs::File::open(truncated.path())?,
                    reference.clone()
                ))
                .is_err()
        );
        identity.pe.timestamp ^= 1;
        assert!(identity.verify_dump(&bytes, &reference).is_err());
        identity.pe.timestamp ^= 1;
        identity.pe.machine = 0x14c;
        assert!(identity.verify_dump(&bytes, &reference).is_err());
        identity.pe.machine = 0x8664;
        identity.pe.codeview.as_mut().unwrap().guid = "0".repeat(32);
        assert!(identity.verify_dump(&bytes, &reference).is_err());
        let oversized = Reference {
            size: MAX_DUMP + 1,
            ..reference
        };
        assert!(
            identity
                .admit_dump(&mut AdmittedInput::new(
                    fs::File::open(file.path())?,
                    oversized
                ))
                .is_err()
        );
        Ok(())
    }
    fn module(identity: &Identity) -> Value {
        let cv = identity.pe.codeview.as_ref().unwrap();
        json!({"image_name":basename(&identity.image.path),"timestamp":identity.pe.timestamp,
            "size":identity.pe.size,"checksum":identity.pe.checksum,"user_mode":true,
            "symbols":"pdb","pdb":{"guid":cv.guid,"age":cv.age}})
    }
    #[test]
    fn real_linked_image_and_pdb_correspond_and_staging_readers_rewind() -> Result<()> {
        let mut files = fixture()?;
        let identity = Identity::admit(&mut files)?;
        assert!(identity.has_symbols());
        assert_eq!(identity.pe.machine, 0x8664);
        for file in &mut files {
            assert_eq!(file.stream_position()?, 0);
        }
        let mut magic = [0; 2];
        files[0].read_exact(&mut magic)?;
        assert_eq!(&magic, b"MZ");
        let value = module(&identity);
        identity.observed(&json!({"modules":[value],"matched":1,"loaded":1}))?;
        Ok(())
    }
    #[test]
    fn crash_locations_bind_admitted_content_and_rva_without_following_module_paths() -> Result<()>
    {
        let identity = Identity::admit(&mut fixture()?)?;
        let location = |base: u64| -> Result<Option<Value>> {
            let mut observed = module(&identity);
            observed["start"] = json!(format!("0x{base:016x}"));
            identity.normalize(
                base + 32,
                &json!({"modules":[observed],"matched":1,"loaded":1}),
            )
        };
        assert_eq!(location(0x100000)?, location(0x200000)?);
        assert_eq!(
            location(0x100000)?.unwrap()["module_sha256"],
            identity.image.sha256
        );
        let mut observed = module(&identity);
        observed["start"] = json!("0x0000000000100000");
        let listing = json!({"modules":[observed],"matched":1,"loaded":1});
        assert!(identity.normalize(0x42, &listing)?.is_none());
        assert!(
            identity
                .normalize(0x100000 + u64::from(identity.pe.size), &listing)?
                .is_none()
        );
        let mut changed = listing;
        changed["modules"][0]["checksum"] = json!(identity.pe.checksum ^ 1);
        assert!(identity.normalize(0x100020, &changed).is_err());
        Ok(())
    }
    #[test]
    fn rejects_changed_image_symbols_and_ambiguous_engine_inventory() -> Result<()> {
        let identity = Identity::admit(&mut fixture()?)?;
        let valid = module(&identity);
        for (field, value) in [
            ("image_name", json!("another.exe")),
            ("timestamp", json!(identity.pe.timestamp ^ 1)),
            ("size", json!(identity.pe.size + 1)),
            ("checksum", json!(identity.pe.checksum ^ 1)),
            ("user_mode", json!(false)),
            ("unloaded", json!(true)),
            ("symbols", json!("deferred")),
            ("pdb", Value::Null),
        ] {
            let mut changed = valid.clone();
            changed[field] = value;
            assert!(identity.verify_module(&changed).is_err(), "{field}");
        }
        for (field, value) in [
            ("guid", json!("0".repeat(32))),
            ("age", json!(0)),
            ("unmatched", json!(true)),
        ] {
            let mut changed = valid.clone();
            changed["pdb"][field] = value;
            assert!(identity.verify_module(&changed).is_err(), "PDB {field}");
        }
        for listing in [
            json!({"modules":[],"matched":0,"loaded":0}),
            json!({"modules":[valid],"matched":2,"loaded":2}),
            json!({"modules":[valid],"matched":1,"loaded":2}),
            json!({"modules":[valid,valid],"matched":2,"loaded":2}),
        ] {
            assert!(identity.observed(&listing).is_err());
        }
        Ok(())
    }
    #[test]
    fn linker_age_controls_correspondence_when_information_age_has_advanced() -> Result<()> {
        let mut identity = Identity::admit(&mut fixture()?)?;
        let pdb = identity.pdb.as_mut().unwrap();
        pdb.information_age += 1;
        matches_symbols(&identity.pe, pdb)?;
        let original = pdb.linker_age;
        pdb.linker_age = Some(pdb.information_age);
        assert!(matches_symbols(&identity.pe, pdb).is_err());
        pdb.linker_age = original;
        pdb.machine ^= 1;
        assert!(matches_symbols(&identity.pe, pdb).is_err());
        pdb.machine ^= 1;
        pdb.guid = "0".repeat(32);
        assert!(matches_symbols(&identity.pe, pdb).is_err());
        Ok(())
    }
    #[test]
    fn bounded_source_refuses_outside_overflow_and_budget_before_library_allocation() -> Result<()>
    {
        use pdb::Source;
        let mut temp = tempfile::NamedTempFile::new()?;
        temp.write_all(&[0; 4096])?;
        let mut input = admitted(temp.path(), "files/test.pdb", false)?;
        let mut source = PdbSource {
            input: &mut input,
            remaining: 8,
        };
        for slice in [
            pdb::SourceSlice {
                offset: u64::MAX,
                size: 2,
            },
            pdb::SourceSlice {
                offset: 4095,
                size: 2,
            },
            pdb::SourceSlice { offset: 0, size: 9 },
            pdb::SourceSlice {
                offset: 0,
                size: usize::MAX,
            },
        ] {
            assert!(source.view(&[slice]).is_err());
            assert_eq!(source.remaining, 8);
        }
        assert_eq!(
            source
                .view(&[pdb::SourceSlice { offset: 0, size: 8 }])?
                .as_slice(),
            &[0; 8]
        );
        assert!(
            source
                .view(&[pdb::SourceSlice { offset: 0, size: 1 }])
                .is_err()
        );
        assert!(symbols(&mut input).is_err());
        assert!(pe(b"MZ").is_err());
        Ok(())
    }
    #[test]
    fn absent_symbols_are_explicit_and_embedded_paths_never_select_other_directories() -> Result<()>
    {
        let mut files = fixture()?;
        let mut reference = files[1].reference().clone();
        reference.path.insert_str(6, "elsewhere/");
        files[1] = AdmittedInput::new(
            fs::File::open(std::env::current_exe()?.with_extension("pdb"))?,
            reference,
        );
        let identity = Identity::admit(&mut files)?;
        assert!(!identity.has_symbols());
        let mut value = module(&identity);
        value["symbols"] = json!("deferred");
        value.as_object_mut().unwrap().remove("pdb");
        identity.verify_module(&value)?;
        Ok(())
    }
}
