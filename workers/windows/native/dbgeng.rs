//! One thread owns one DbgEng session. Only SetInterrupt crosses that thread boundary.
use anyhow::{Context, Result, bail, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde_json::{Value, json};
use std::{
    ffi::c_void,
    io::{BufRead, Read, Write},
    mem::ManuallyDrop,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};
use windows::{
    Win32::{
        Foundation::{CloseHandle, GetLastError, HANDLE},
        Security::{
            AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW,
            SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
        },
        System::{
            Diagnostics::Debug::Extensions::*,
            LibraryLoader::{
                GetProcAddress, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
                LoadLibraryExW,
            },
            Threading::{GetCurrentProcess, OpenProcessToken},
        },
    },
    core::{Interface, PCWSTR, implement, s, w},
};

const MAX_REQUEST: u64 = 1024 * 1024;
const MAX_OUTPUT: usize = 64 * 1024;

#[derive(Default)]
struct OutputLog {
    text: String,
    truncated: bool,
}

#[implement(IDebugOutputCallbacksWide)]
struct Output {
    log: Arc<Mutex<OutputLog>>,
}

impl IDebugOutputCallbacksWide_Impl for Output_Impl {
    fn Output(&self, _mask: u32, text: &PCWSTR) -> windows::core::Result<()> {
        // DbgEng owns a valid NUL-terminated UTF-16 string for this callback.
        let source = unsafe { text.as_wide() };
        let bounded = String::from_utf16_lossy(&source[..source.len().min(MAX_OUTPUT)]);
        let mut log = self.log.lock().unwrap_or_else(|e| e.into_inner());
        let available = MAX_OUTPUT.saturating_sub(log.text.len());
        let mut end = available.min(bounded.len());
        while !bounded.is_char_boundary(end) {
            end -= 1;
        }
        log.text.push_str(&bounded[..end]);
        log.truncated |= end < bounded.len() || source.len() > MAX_OUTPUT;
        Ok(())
    }
}

fn wide(value: &str) -> Result<Vec<u16>> {
    ensure!(
        !value.contains('\0'),
        "NUL is not permitted in a debugger path or symbol"
    );
    Ok(value.encode_utf16().chain(Some(0)).collect())
}

fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .with_context(|| format!("{key} must be a string"))
}

fn integer(value: &Value, key: &str, default: u64, max: u64) -> Result<u64> {
    let result = match value.get(key) {
        None => default,
        Some(v) => v
            .as_u64()
            .with_context(|| format!("{key} must be an unsigned integer"))?,
    };
    ensure!(result <= max, "{key} exceeds the operation limit");
    Ok(result)
}

fn local_path(path: &Path) -> Result<PathBuf> {
    use std::os::windows::fs::MetadataExt;
    // Normalized-name queries require a privilege removed from the ordinary
    // worker token. Keep the verified absolute identity and inspect every
    // component instead of restoring privileges or following reparse points.
    let name = path
        .to_str()
        .context("DbgEng paths must be Unicode")?
        .replace('/', "\\");
    let drive = name.strip_prefix(r"\\?\").unwrap_or(&name);
    ensure!(
        drive.as_bytes().get(1) == Some(&b':')
            && drive.as_bytes()[0].is_ascii_alphabetic()
            && !drive.contains([';', '\0'])
            && !drive
                .split('\\')
                .skip(1)
                .any(|part| part.ends_with('.') || part.ends_with(' ') || part.contains(':')),
        "DbgEng requires an unambiguous local drive path"
    );
    let path = PathBuf::from(drive);
    ensure!(
        path.is_absolute()
            && !path.components().any(|component| matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )),
        "DbgEng requires an absolute local path without relative aliases"
    );
    for current in path.ancestors().filter(|p| !p.as_os_str().is_empty()) {
        let metadata = std::fs::symlink_metadata(current)?;
        ensure!(
            !metadata.file_type().is_symlink() && metadata.file_attributes() & 0x400 == 0,
            "DbgEng paths cannot contain links or reparse points"
        );
    }
    // DbgEng treats even a local \\?\ drive path as a network path when network
    // access is disabled. Convert only unambiguous drive paths, preserving identity.
    Ok(path)
}

pub struct Engine {
    client: IDebugClient5,
    control: IDebugControl4,
    symbols: IDebugSymbols3,
    registers: IDebugRegisters2,
    memory: IDebugDataSpaces,
    systems: IDebugSystemObjects,
    output: Arc<Mutex<OutputLog>>,
    mode: String,
    invalidated: bool,
    debug_privilege: bool,
    engine_path: PathBuf,
}

impl Engine {
    fn new(directory: &Path, symbol_directory: &Path) -> Result<Self> {
        let directory = local_path(directory).context("resolve local DbgEng directory")?;
        let engine_path = directory.join("dbgeng.dll");
        let encoded = wide(&engine_path.to_string_lossy())?;
        // The helper process owns this module for its entire lifetime. Do not unload
        // it while DbgEng may retain internal threads or COM references.
        let library = unsafe {
            LoadLibraryExW(
                PCWSTR(encoded.as_ptr()),
                None,
                LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
            )
        }
        .context("load packaged DbgEng library")?;
        let entry = unsafe { GetProcAddress(library, s!("DebugCreate")) }
            .context("DbgEng does not export DebugCreate")?;
        type Create = unsafe extern "system" fn(
            *const windows::core::GUID,
            *mut *mut c_void,
        ) -> windows::core::HRESULT;
        // DebugCreate has this documented ABI; the explicit module path selects the engine.
        let create: Create = unsafe { std::mem::transmute(entry) };
        let mut raw = std::ptr::null_mut();
        unsafe {
            create(&IDebugClient5::IID, &mut raw)
                .ok()
                .context("initialize DbgEng client")?;
        }
        ensure!(!raw.is_null(), "DbgEng returned an empty client");
        let client = unsafe { IDebugClient5::from_raw(raw) };
        let control: IDebugControl4 = client.cast().context("query DbgEng control interface")?;
        let symbols: IDebugSymbols3 = client.cast().context("query DbgEng symbol interface")?;
        let output = Arc::new(Mutex::new(OutputLog::default()));
        let callback: IDebugOutputCallbacksWide = Output {
            log: output.clone(),
        }
        .into();
        let symbols_path =
            local_path(symbol_directory).context("resolve local DbgEng symbol directory")?;
        let encoded_symbols = wide(&symbols_path.to_string_lossy())?;
        unsafe {
            client
                .SetOutputCallbacksWide(&callback)
                .context("set DbgEng output callback")?;
            control
                .AddEngineOptions(
                    DEBUG_ENGOPT_DISALLOW_NETWORK_PATHS
                        | DEBUG_ENGOPT_DISALLOW_SHELL_COMMANDS
                        | DEBUG_ENGOPT_DISABLESQM
                        | DEBUG_ENGOPT_INITIAL_BREAK,
                )
                .context("set local-only DbgEng engine options")?;
            symbols
                .SetSymbolPathWide(PCWSTR(encoded_symbols.as_ptr()))
                .context("set local-only DbgEng symbol path")?;
            // Exact local symbols only; ignore image-embedded search paths and
            // disable SymSrv auto-detection. SYMOPT_SECURE is the kernel-only
            // restricted debugger mode and prevents live user-mode attachment.
            symbols
                .SetSymbolOptions(
                    0x200 | 0x80000 | 0x400 | 0x4 | 0x1000 | 0x20000 | 0x200000 | 0x2000000,
                )
                .context("set local-only DbgEng symbol options")?;
            symbols
                .SetSourcePathWide(PCWSTR(wide("")?.as_ptr()))
                .context("clear DbgEng source search path")?;
        }
        Ok(Self {
            registers: client.cast().context("query DbgEng register interface")?,
            memory: client.cast().context("query DbgEng memory interface")?,
            systems: client.cast().context("query DbgEng system interface")?,
            client,
            control,
            symbols,
            output,
            mode: "uninitialized".into(),
            invalidated: false,
            debug_privilege: false,
            engine_path,
        })
    }

    fn wait(&mut self, milliseconds: u32) -> Result<Value> {
        let interrupt: IDebugControl = self.control.cast()?;
        let pointer = interrupt.as_raw() as usize;
        let deadline = AtomicBool::new(false);
        let (stop, stopped) = mpsc::channel();
        let result = std::thread::scope(|scope| {
            let deadline = &deadline;
            scope.spawn(move || {
                if stopped
                    .recv_timeout(Duration::from_millis(milliseconds.into()))
                    .is_err()
                {
                    deadline.store(true, Ordering::SeqCst);
                    // Microsoft explicitly permits SetInterrupt from any thread. All
                    // other DbgEng methods stay on the session's owning thread. The
                    // borrowed interface remains alive until this scoped thread joins.
                    unsafe {
                        signal_interrupt(pointer, DEBUG_INTERRUPT_ACTIVE);
                    }
                    if stopped.recv_timeout(Duration::from_secs(2)).is_err() {
                        unsafe {
                            signal_interrupt(pointer, DEBUG_INTERRUPT_EXIT);
                        }
                    }
                }
            });
            let result = unsafe { self.control.WaitForEvent(0, u32::MAX) };
            let _ = stop.send(());
            result
        });
        let observed = unsafe { self.control.GetExecutionStatus() };
        if result.is_err()
            && !observed
                .as_ref()
                .is_ok_and(|state| *state == DEBUG_STATUS_NO_DEBUGGEE)
        {
            self.invalidated = true;
            result.context("debugger wait failed; session invalidated")?;
        }
        let state = match observed {
            Ok(state) => state,
            Err(error) => {
                self.invalidated = true;
                return Err(error).context("debugger execution state is unknown");
            }
        };
        Ok(
            json!({"execution_status":state,"execution_state":state_name(state),
            "interruption":if deadline.load(Ordering::SeqCst) {Some("deadline")} else {None}}),
        )
    }

    fn open(&mut self, params: &Value) -> Result<Value> {
        ensure!(
            self.mode == "uninitialized",
            "helper already owns a debugging session"
        );
        // Any failed open invalidates the helper; no partially opened dump or
        // attachment may remain available through subsequent inspect requests.
        self.invalidated = true;
        let timeout = integer(params, "timeout_secs", 30, 60)?.max(1) as u32 * 1000;
        match text(params, "mode")? {
            "dump" => {
                let path = local_path(Path::new(text(params, "dump")?))?;
                let encoded = wide(&path.to_string_lossy())?;
                unsafe {
                    self.client.OpenDumpFileWide(PCWSTR(encoded.as_ptr()), 0)?;
                }
                self.mode = "dump".into();
            }
            "attach" => {
                let pid = integer(params, "pid", 0, u32::MAX as u64)? as u32;
                ensure!(pid != 0, "owned process ID is required");
                self.debug_privilege = enable_debug_privilege();
                unsafe {
                    self.client
                        .AttachProcess(
                            0,
                            pid,
                            DEBUG_ATTACH_DEFAULT | DEBUG_ATTACH_INVASIVE_RESUME_PROCESS,
                        )
                        .context("attach worker-owned process")?;
                }
                self.mode = "live".into();
            }
            _ => bail!("helper accepts dump or worker-owned process attachment"),
        }
        let state = self.wait(timeout)?;
        let mut debuggee_class = 0;
        let mut debuggee_qualifier = 0;
        unsafe {
            self.control
                .GetDebuggeeType(&mut debuggee_class, &mut debuggee_qualifier)?;
        }
        ensure!(
            debuggee_class == DEBUG_CLASS_USER_WINDOWS,
            "only Windows user-mode debugging is supported; kernel and other debuggee classes are rejected"
        );
        if self.mode == "dump" {
            ensure!(
                matches!(
                    debuggee_qualifier,
                    DEBUG_USER_WINDOWS_SMALL_DUMP | DEBUG_USER_WINDOWS_DUMP
                ),
                "only Windows user-mode minidumps and full dumps are supported"
            );
        }
        unsafe {
            self.control.SetCodeLevel(DEBUG_LEVEL_ASSEMBLY)?;
        }
        if params["architecture"].as_str() == Some("x86") {
            unsafe {
                self.control.SetEffectiveProcessorType(0x14c)?;
            }
        }
        let architecture = unsafe { self.control.GetEffectiveProcessorType()? };
        self.invalidated = false;
        Ok(
            json!({"state":state,"architecture":architecture,"engine_path":self.engine_path,
            "mode":self.mode,"debuggee_class":debuggee_class,"debuggee_qualifier":debuggee_qualifier,
            "debug_privilege_enabled":self.debug_privilege,"event":self.event()}),
        )
    }

    fn event(&self) -> Value {
        let mut kind = 0;
        let mut pid = 0;
        let mut tid = 0;
        let mut detail = [0u8; 4096];
        let mut description = [0u16; 2048];
        let mut used = 0;
        let result = unsafe {
            self.control.GetLastEventInformationWide(
                &mut kind,
                &mut pid,
                &mut tid,
                Some(detail.as_mut_ptr().cast()),
                detail.len() as u32,
                Some(&mut used),
                Some(&mut description),
                None,
            )
        };
        match result {
            Ok(()) => json!({"type":kind,"process_id":pid,"thread_id":tid,
                "description":utf16(&description),"extra_base64":BASE64.encode(&detail[..(used as usize).min(detail.len())])}),
            Err(error) => json!({"available":false,"error":error.to_string()}),
        }
    }

    fn inspect(&self, params: &Value) -> Result<Value> {
        ensure!(
            self.mode != "uninitialized" && !self.invalidated,
            "session is absent or invalidated"
        );
        let kind = text(params, "kind")?;
        let limit = integer(params, "limit", 64, 256)?.max(1) as usize;
        let data = unsafe {
            match kind {
                "state" => {
                    let state = self.control.GetExecutionStatus()?;
                    json!({"execution_status":state,"execution_state":state_name(state)})
                }
                "exception" => self.event(),
                "registers" => {
                    let count = (self.registers.GetNumberRegisters()? as usize).min(limit);
                    let mut values = Vec::with_capacity(count);
                    for index in 0..count {
                        let mut name = [0u16; 256];
                        self.registers.GetDescriptionWide(
                            index as u32,
                            Some(&mut name),
                            None,
                            None,
                        )?;
                        let mut value = DEBUG_VALUE::default();
                        self.registers.GetValue(index as u32, &mut value)?;
                        let raw = value.Anonymous.RawBytes;
                        let number = match value.Type {
                            DEBUG_VALUE_INT8 => Some(value.Anonymous.I8 as u64),
                            DEBUG_VALUE_INT16 => Some(value.Anonymous.I16 as u64),
                            DEBUG_VALUE_INT32 => Some(value.Anonymous.I32 as u64),
                            DEBUG_VALUE_INT64 => Some(value.Anonymous.Anonymous.I64),
                            _ => None,
                        };
                        values.push(json!({"number":index,"name":utf16(&name),"type":value.Type,"value":number,"raw_hex":hex::encode(raw)}));
                    }
                    json!({"registers":values})
                }
                "stack" => {
                    let mut frames = vec![DEBUG_STACK_FRAME::default(); limit];
                    let mut count = 0;
                    self.control
                        .GetStackTrace(0, 0, 0, &mut frames, Some(&mut count))?;
                    let frames: Vec<_> = frames[..(count as usize).min(limit)].iter().map(|frame| json!({
                    "level":frame.FrameNumber,"instruction_offset":frame.InstructionOffset,"return_offset":frame.ReturnOffset,
                    "stack_offset":frame.StackOffset,"frame_offset":frame.FrameOffset,
                    "symbol":self.symbol(frame.InstructionOffset),
                })).collect();
                    json!({"frames":frames})
                }
                "memory" => {
                    let address = integer(params, "address", 0, u64::MAX)?;
                    let length = integer(params, "length", 128, 65536)?.max(1) as usize;
                    ensure!(
                        address.checked_add(length as u64).is_some(),
                        "address range overflows"
                    );
                    let mut bytes = vec![0u8; length];
                    let mut read = 0;
                    self.memory.ReadVirtual(
                        address,
                        bytes.as_mut_ptr().cast(),
                        length as u32,
                        Some(&mut read),
                    )?;
                    ensure!(
                        read as usize <= length,
                        "engine returned invalid memory length"
                    );
                    json!({"address":address,"bytes_read":read,"data_base64":BASE64.encode(&bytes[..read as usize])})
                }
                "threads" => {
                    let count = (self.systems.GetNumberThreads()? as usize).min(limit);
                    let mut ids = vec![0; count];
                    let mut systems = vec![0; count];
                    self.systems.GetThreadIdsByIndex(
                        0,
                        count as u32,
                        Some(ids.as_mut_ptr()),
                        Some(systems.as_mut_ptr()),
                    )?;
                    json!({"threads":ids.into_iter().zip(systems).map(|(id,system)|json!({"id":id,"system_id":system})).collect::<Vec<_>>()})
                }
                "modules" => {
                    let mut loaded = 0;
                    let mut unloaded = 0;
                    self.symbols.GetNumberModules(&mut loaded, &mut unloaded)?;
                    let mut modules = Vec::new();
                    for index in 0..(loaded as usize).min(limit) {
                        let base = self.symbols.GetModuleByIndex(index as u32)?;
                        let mut image = [0u16; 2048];
                        let mut module = [0u16; 256];
                        let mut parameters = DEBUG_MODULE_PARAMETERS::default();
                        self.symbols.GetModuleNameStringWide(
                            DEBUG_MODNAME_IMAGE,
                            index as u32,
                            base,
                            Some(&mut image),
                            None,
                        )?;
                        self.symbols.GetModuleNameStringWide(
                            DEBUG_MODNAME_MODULE,
                            index as u32,
                            base,
                            Some(&mut module),
                            None,
                        )?;
                        self.symbols
                            .GetModuleParameters(1, Some(&base), 0, &mut parameters)?;
                        modules.push(json!({"base":base,"size":parameters.Size,"timestamp":parameters.TimeDateStamp,
                        "checksum":parameters.Checksum,"symbol_type":parameters.SymbolType,
                        "image":utf16(&image),"name":utf16(&module)}));
                    }
                    json!({"modules":modules,"loaded_count":loaded,"unloaded_count":unloaded})
                }
                "disassembly" => {
                    let address = integer(params, "address", 0, u64::MAX)?;
                    let length = integer(params, "length", 128, 65536)?.max(1);
                    let end = address
                        .checked_add(length)
                        .context("address range overflows")?;
                    let mut cursor = address;
                    let mut lines = Vec::new();
                    while cursor < end && lines.len() < limit {
                        let mut buffer = [0u16; 2048];
                        let mut next = 0;
                        self.control.DisassembleWide(
                            cursor,
                            0,
                            Some(&mut buffer),
                            None,
                            &mut next,
                        )?;
                        ensure!(next > cursor, "disassembler did not advance");
                        lines.push(json!({"address":cursor,"instruction":utf16(&buffer),"next_address":next}));
                        cursor = next;
                    }
                    json!({"instructions":lines})
                }
                _ => bail!("unsupported debugger inspection"),
            }
        };
        Ok(json!({"kind":kind,"data":data}))
    }

    unsafe fn symbol(&self, address: u64) -> Option<Value> {
        let mut name = [0u16; 1024];
        let mut displacement = 0;
        unsafe {
            self.symbols
                .GetNameByOffsetWide(address, Some(&mut name), None, Some(&mut displacement))
                .ok()?;
        }
        Some(json!({"name":utf16(&name),"displacement":displacement}))
    }

    fn control(&mut self, params: &Value) -> Result<Value> {
        ensure!(
            self.mode == "live" && !self.invalidated,
            "execution control requires a live owned process"
        );
        let action = text(params, "action")?;
        match action {
            "breakpoint_add" => {
                let location = if let Some(address) = params["address"].as_u64() {
                    address
                } else {
                    let symbol = wide(text(params, "symbol")?)?;
                    unsafe { self.symbols.GetOffsetByNameWide(PCWSTR(symbol.as_ptr()))? }
                };
                unsafe {
                    let breakpoint = ManuallyDrop::new(
                        self.control
                            .AddBreakpoint(DEBUG_BREAKPOINT_CODE, u32::MAX)?,
                    );
                    breakpoint.SetOffset(location)?;
                    breakpoint.AddFlags(DEBUG_BREAKPOINT_ENABLED)?;
                    return Ok(
                        json!({"action":action,"breakpoint_id":breakpoint.GetId()?.to_string(),"address":location}),
                    );
                }
            }
            "breakpoint_remove" | "breakpoint_enable" | "breakpoint_disable" => {
                let id: u32 = text(params, "breakpoint_id")?
                    .parse()
                    .context("invalid breakpoint ID")?;
                unsafe {
                    // DbgEng owns breakpoint lifetime independently of IUnknown;
                    // RemoveBreakpoint deletes it immediately. Never Release it.
                    let breakpoint = ManuallyDrop::new(self.control.GetBreakpointById(id)?);
                    match action {
                        "breakpoint_remove" => self.control.RemoveBreakpoint(&*breakpoint)?,
                        "breakpoint_enable" => breakpoint.AddFlags(DEBUG_BREAKPOINT_ENABLED)?,
                        _ => breakpoint.RemoveFlags(DEBUG_BREAKPOINT_ENABLED)?,
                    }
                }
            }
            "interrupt" => unsafe {
                if self.control.GetExecutionStatus()? != DEBUG_STATUS_BREAK {
                    self.control.SetInterrupt(DEBUG_INTERRUPT_ACTIVE)?;
                }
            },
            "run" | "continue" | "step" | "next" | "finish" => {
                let _temporary = if action == "finish" {
                    let mut frames = [DEBUG_STACK_FRAME::default()];
                    let mut count = 0;
                    unsafe {
                        self.control
                            .GetStackTrace(0, 0, 0, &mut frames, Some(&mut count))?;
                    }
                    ensure!(
                        count == 1 && frames[0].ReturnOffset != 0,
                        "current frame has no return address"
                    );
                    let breakpoint = ManuallyDrop::new(unsafe {
                        self.control
                            .AddBreakpoint(DEBUG_BREAKPOINT_CODE, u32::MAX)?
                    });
                    let temporary = TemporaryBreakpoint {
                        control: self.control.clone(),
                        id: unsafe { breakpoint.GetId()? },
                    };
                    unsafe {
                        breakpoint.SetOffset(frames[0].ReturnOffset)?;
                        breakpoint
                            .AddFlags(DEBUG_BREAKPOINT_ENABLED | DEBUG_BREAKPOINT_ONE_SHOT)?;
                    }
                    Some(temporary)
                } else {
                    None
                };
                let status = match action {
                    "step" => DEBUG_STATUS_STEP_INTO,
                    "next" => DEBUG_STATUS_STEP_OVER,
                    _ => DEBUG_STATUS_GO,
                };
                unsafe {
                    self.control.SetExecutionStatus(status)?;
                }
                let state =
                    self.wait(integer(params, "timeout_secs", 30, 60)?.max(1) as u32 * 1000)?;
                return Ok(json!({"action":action,"state":state,"event":self.event()}));
            }
            _ => bail!("unsupported debugger control"),
        }
        Ok(json!({"action":action,"event":self.event()}))
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.EndSession(if self.mode == "live" {
                DEBUG_END_ACTIVE_TERMINATE
            } else {
                DEBUG_END_PASSIVE
            });
        }
    }
}

struct TemporaryBreakpoint {
    control: IDebugControl4,
    id: u32,
}
impl Drop for TemporaryBreakpoint {
    fn drop(&mut self) {
        unsafe {
            // A hit one-shot breakpoint has already been deleted by the engine.
            if let Ok(breakpoint) = self.control.GetBreakpointById(self.id) {
                let breakpoint = ManuallyDrop::new(breakpoint);
                let _ = self.control.RemoveBreakpoint(&*breakpoint);
            }
        }
    }
}

// The caller keeps the interface alive and joins the watchdog before returning.
unsafe fn signal_interrupt(pointer: usize, flags: u32) {
    let raw = pointer as *mut c_void;
    let table = unsafe { *(raw as *mut *const IDebugControl_Vtbl) };
    let _ = unsafe { ((*table).SetInterrupt)(raw, flags) };
}

fn enable_debug_privilege() -> bool {
    // Enable an already-granted privilege in this short-lived helper only. This
    // never grants a user right or changes the parent/controller's token.
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &mut token,
        )
        .is_err()
        {
            return false;
        }
        let mut luid = Default::default();
        let enabled = if LookupPrivilegeValueW(None, w!("SeDebugPrivilege"), &mut luid).is_ok() {
            let privileges = TOKEN_PRIVILEGES {
                PrivilegeCount: 1,
                Privileges: [LUID_AND_ATTRIBUTES {
                    Luid: luid,
                    Attributes: SE_PRIVILEGE_ENABLED,
                }],
            };
            AdjustTokenPrivileges(token, false, Some(&privileges), 0, None, None).is_ok()
                && GetLastError().0 == 0
        } else {
            false
        };
        let _ = CloseHandle(token);
        enabled
    }
}

fn utf16(value: &[u16]) -> String {
    let end = value.iter().position(|v| *v == 0).unwrap_or(value.len());
    String::from_utf16_lossy(&value[..end])
}

fn state_name(state: u32) -> &'static str {
    match state & DEBUG_STATUS_MASK {
        DEBUG_STATUS_BREAK => "stopped",
        DEBUG_STATUS_NO_DEBUGGEE => "exited",
        _ => "running",
    }
}

pub fn serve() -> Result<()> {
    let arguments: Vec<_> = std::env::args_os().collect();
    ensure!(
        arguments.len() == 3,
        "usage: wukong-dbgeng <engine-directory> <local-symbol-directory>"
    );
    let mut engine = Engine::new(Path::new(&arguments[1]), Path::new(&arguments[2]))?;
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    loop {
        let mut line = Vec::new();
        let count = input
            .by_ref()
            .take(MAX_REQUEST + 1)
            .read_until(b'\n', &mut line)?;
        if count == 0 {
            break;
        }
        ensure!(
            count as u64 <= MAX_REQUEST && line.last() == Some(&b'\n'),
            "helper request exceeds 1 MiB or lacks a newline"
        );
        let request: Value = serde_json::from_slice(&line)?;
        let operation = text(&request, "operation")?;
        let result = match operation {
            "doctor" => Ok(
                json!({"engine":"dbgeng","protocol":1,"engine_path":engine.engine_path,
                "capabilities":["dump","owned_process_attach","inspect","control"],"local_symbols_only":true}),
            ),
            "open" => engine.open(&request),
            "inspect" => engine.inspect(&request),
            "control" => engine.control(&request),
            "close" => Ok(json!({"state":"closed"})),
            _ => Err(anyhow::anyhow!("unsupported helper operation")),
        };
        let log = std::mem::take(&mut *engine.output.lock().unwrap_or_else(|e| e.into_inner()));
        let reply = match result {
            Ok(value) => {
                json!({"id":request["id"],"result":value,"output":log.text,"output_truncated":log.truncated})
            }
            Err(error) => {
                json!({"id":request["id"],"error":format!("{error:#}"),"session_invalidated":engine.invalidated,
                "output":log.text,"output_truncated":log.truncated})
            }
        };
        serde_json::to_writer(&mut output, &reply)?;
        output.write_all(b"\n")?;
        output.flush()?;
        if operation == "close" || engine.invalidated {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod path_tests {
    use super::*;
    #[test]
    fn dbgeng_local_paths_preserve_existing_files_and_reject_networks_and_aliases() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("known fixture.bin");
        std::fs::write(&path, b"inert-known-fixture")?;
        assert_eq!(local_path(&path)?, path);
        assert_eq!(
            local_path(Path::new(&path.to_string_lossy().replace('\\', "/")))?,
            path
        );
        for invalid in [
            r"\\server\share\file",
            r"C:\file:stream",
            r"relative\file",
            r"C:\dir.\file",
            r"C:\dir\..\file",
        ] {
            assert!(local_path(Path::new(invalid)).is_err());
        }
        Ok(())
    }
}
