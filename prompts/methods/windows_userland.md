# Windows userland vulnerability research

Use when the assigned target is a Windows PE (x86 or x64) userland program and
the question concerns a security boundary: parser or format handling, IPC,
privileges, memory ownership or a crash. Kernel drivers, hypervisor behavior,
GUI automation and exploit reliability beyond the owned process stay outside
this method and outside the current Windows worker contract.

## Establish the environment first

Only a registered Windows VM worker executes or debugs PE. Call
`environment_doctor` before promising runtime work; without that backend stay
static and say the execution question is unanswered.

On a qualified worker the available surface is `binary_inspect`,
`binary_strings` and the Ghidra tools on the controller, plus `stage_target`,
`file_put`/`file_get`, `job_start`/`job_status`/`job_artifacts`,
`exec_command`, `interactive_start`/`interactive_send`/`interactive_close`,
`debug_start`, `debug_inspect`, `debug_control`, `debug_close`,
`crash_replay`, `input_minimize`, `poc_verify`, `py_run`, `differential_test`
and `runtime_profile`. Only a verifier may use `verification_record`.

Not available on Windows: `fuzz_start` (AFL++ QEMU is a Linux worker method;
`fuzz_status` can only observe an already bound job), `debug_command` (GDB/MI
only), `crash_snapshot` (no `core.snapshot`), `runtime_prepare`,
`cyclic_offset`, `libc_identify`, `emulate_run`, `angr_solve` and
`artifact_validate`. There is no coverage-guided campaign; discovery is bounded
and hypothesis-driven. Capacity is one active resource, so close a debugger or
interactive session before starting another job.

## Static facts that change a hypothesis

`binary_inspect` reports sections, imports, symbols, image base and PE DLL
characteristics. Treat `NX_COMPAT` as DEP opt-in, `DYNAMIC_BASE` as ASLR
opt-in and `GUARD_CF` as CFG instrumentation, not as proof of loader policy.
High-entropy VA, CET and ACG are not reported, so keep them unknown. Ghidra
addresses are module-relative: subtract `image_base` from a link-time VA, and
never pass a file offset or page number.

Map imports to trust boundaries rather than conclusions:

- execution: `CreateProcess*`, `ShellExecute*`, `WinExec`, `LoadLibrary*`,
  `GetProcAddress`;
- services and privileges: `OpenSCManager*`, `CreateService*`,
  `OpenProcessToken`, `GetTokenInformation`, `SetTokenInformation`,
  `AdjustTokenPrivileges`, `LookupPrivilegeValue*`, `ImpersonateLoggedOnUser`,
  `LogonUser*`;
- IPC: `CreateNamedPipe*`, `ConnectNamedPipe`, `TransactNamedPipe`,
  `CoCreateInstance*`, `CoInitializeSecurity`, `RpcServerRegisterIf*`;
- memory: `HeapAlloc`/`HeapCreate`/`RtlAllocateHeap`, `VirtualAlloc`,
  `MapViewOfFile`;
- hardening: `__security_cookie`/`__security_check_cookie`,
  `_guard_check_icall`/`_guard_dispatch_icall`.

`/GS` symbols prove use somewhere, not per-function coverage; check the actual
prologue, epilogue and failure path. x64 exception handling is table-based,
while x86 `SafeSEH` and handler records need separate checks. A dangerous
import or a crash alone is not a vulnerability.

## Hypotheses by boundary

- Parsers and file formats: length, offset, sign and width invariants, integer
  truncation in 32-bit size fields, UTF-16/ANSI length confusion, path
  canonicalization (`\\?\`, `..`, trailing dot or space, alternate data
  streams).
- Memory ownership: NT heap (LFH/segment heap) and lookaside reuse, double
  free, use-after-free, stale handles, reference counts, VEH and callback
  lifetimes.
- IPC: named-pipe impersonation and DACL/session isolation, message size and ID
  assumptions, COM marshalling, RPC interface reachability; ALPC only when the
  contract includes it.
- Tokens and integrity: requested versus effective privileges, integrity level,
  UAC auto-elevation, service account and AppContainer boundaries. Observe
  behavior or object security, not just API names.
- Execution sinks: command-line quoting, DLL/image search order, environment and
  current-directory influence, COM hijack, service binary paths.

Select one promising candidate, state the input origin, controlled fields, path
guards, violated invariant and trigger preconditions, then look for a
counterexample before filing anything.

## Test within the available evidence

A normal loop is static facts and Ghidra, a hypothesis, `stage_target`, then
`job_start` or `interactive_start` with a small discriminating input,
`debug_start` (launch or a separately staged minidump), `debug_inspect` or
`debug_control`, `crash_replay`, `input_minimize` and `poc_verify`.

- `debug_start` selects DbgEng on Windows and never falls back to another
  platform; dump sessions reject execution control.
- `input_minimize` needs a fresh stopped-session observation with a strong
  signature: admitted image SHA256, exception code, image-relative PC and
  resolved frames. With only a signal or exception code, set
  `allow_signal_only` and label the equivalence weak.
- `poc_verify` on native Windows replays argv/stdin only; do not plan a network
  replay there.
- `py_run` is isolated CPython without pwntools, capstone or Unicorn. Use it for
  byte layout, encoding and input generation, not for pwntools-style
  exploitation or coverage measurement.
- `differential_test` compares bounded stdin vectors between the target and a
  reference build; a timeout or truncated output is inconclusive.
- `runtime_profile` and `debug_inspect` observe mappings and module paths.
  Mapped module contents stay unknown on Windows: the admitted file hash is
  evidence about the file, not about the mapping.

Do not describe a manual corpus as fuzzing, and do not claim coverage that was
not measured.

## Evidence, verification and limits

Record target SHA256, launch arguments, injected bytes, OS build, session IDs
and what could not be observed, including missing symbols/PDB, mapped hashes
and kernel state. File a finding only at the stage the evidence supports; a
crash is reproduction, not impact. Independent verification belongs to a
separate verifier, which also cannot use `artifact_validate` on Windows.

Out of scope without a new contract and backend: kernel or driver targets,
hypervisor isolation claims, automatic VM snapshots, GUI automation and network
protocol fuzzing.
