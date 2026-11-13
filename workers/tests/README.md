<a id="worker-测试"></a>

# Worker tests

English · [简体中文](README.zh-CN.md)

The ignored native `live_sdk_subscription_opens_and_unsubscribes_on_the_dedicated_vm` fixture opens an actual app/listener-filtered WFP subscription, checks its trusted loopback control and drains the SDK subscription. Run it only as an administrator on a dedicated Windows VM. It changes no firewall rule or global event option and does not establish that a contained packet was blocked. The provided Windows 10 VM passed this SDK fixture; live drop and concurrent cold-start acceptance still require the installed worker's qualification. See the [Windows observation contract](../windows/README.md).

On packages implementing native discovery, set optional `resource_inventory: true` in the process, LaunchSpec or interrupted-process acceptance profile. The shared controller fixture pages both APIs in batches of two, checks exclusive cursor progress and matching IDs, and retains returned resource summaries/statuses in the report. Process acceptance observes a live job; LaunchSpec acceptance requires all six settled jobs, rejects malformed bounds/cursors/scope overrides and verifies that another run sees an empty inventory. Its completed-recovery fixture compares the entire original inventory after an independent service restart. The fault fixture requires the reconciled unknown/closed resource to remain discoverable without reopening. Leave the option absent for historical packages. These additions passed local compilation/library checks and the provided offline VM's installed-service process, LaunchSpec, independent restart and service-interruption acceptance at `ddf86dc`. Source/content identities and original-evidence limits are recorded in the [Windows qualification record](../windows/README.md).

`remote_installed_windows_worker_launch_acceptance` uses `WUKONG_REMOTE_WORKER_LAUNCH_TEST`: managed-SSH `config`, expected `manifest_sha256`, a new absolute `output`, and `probes` with exactly `x64`/`x86`. Each architecture supplies `program` and `dependency`, each with absolute `path` and pinned `sha256`, from the package's `runtime/selftest/` directory. The trusted probe loads its sibling `wukong-fixture.dll`, reads a declared peer outside cwd, writes a new cwd file and copies binary stdin/argv input to stdout. The controller checks six resources, changed manifest/per-call override refusal, full request/receipt identity, cleanup and stable completed starts. No compiler or download runs in the VM. After an independently observed service restart, `remote_native_launch_recovery_acceptance` uses `WUKONG_REMOTE_WORKER_LAUNCH_RECOVERY_TEST` with `config`, absolute `previous`, its `previous_sha256` and a new absolute `output`; it compares four completed LaunchSpec results and original receipts without uploads or execution. These fixtures passed on the provided offline VM at source `6e2bc91`; see the source-bound [qualification record](../windows/README.md).

```powershell
$env:WUKONG_REMOTE_WORKER_LAUNCH_TEST = (Resolve-Path .\remote-launch-profile.json)
cargo test --locked --lib remote_installed_windows_worker_launch_acceptance -- --ignored --nocapture
$env:WUKONG_REMOTE_WORKER_LAUNCH_RECOVERY_TEST = (Resolve-Path .\remote-launch-recovery-profile.json)
cargo test --locked --lib remote_native_launch_recovery_acceptance -- --ignored --nocapture
```

The process profile's optional `argument_files: true` requires the updated `--read-argument` trusted probe. It uploads binary data without executable permission, rejects a missing admitted argument, passes the same logical data path twice, checks the original output bytes/hash and requires exactly one retained native file reference. The contained probe must actually read the file and receive `ERROR_ACCESS_DENIED` for both write and `FILE_EXECUTE` opens. This case passed fresh offline VM installation and service-restart recovery at source `ecc18af`; see report identities and limits in the [Windows qualification record](../windows/README.md).

The ignored Rust fixture `remote_installed_windows_worker_process_acceptance` uses the real controller and an independently pinned, installed Windows VM. Set `WUKONG_REMOTE_WORKER_PROCESS_TEST` to a JSON profile with managed-SSH `config`, expected `manifest_sha256`, a new absolute `output` path and `probes`: exactly `x64`/`x86`, each containing the absolute `path` and SHA256 `sha256` of the trusted self-built `wukong-probe.exe` from that package's `runtime/selftest/` tree. It uploads the bytes through the admitted input protocol; no VM compiler or package download is needed. It checks echo, deadlines, cancellation, one-resource/lease barriers, stable start replay, raw/chunked output hashes and stale fences. The report retains original receipt/output bytes without lease tokens or private keys. This is limited native process acceptance; production capability advertisement remains false.

For schema-2 packages with the updated trusted probe, `cleanup_adversary: true` additionally runs the readonly-file/protected-empty-DACL cleanup case in both architectures. Omit it for older probes. A separately built trusted probe may be pinned for this test without replacing the signed installed worker; record both source identities. Upload paths/request IDs include the probe hash, preserving immutable prior inputs. The cleanup probe initially grants both its actual user and AppContainer SID full access to its own new directory, retains a `READ_CONTROL | WRITE_DAC` handle, then installs and reads back the protected empty DACL through the SDK. A failed ACL setup does not qualify restrictive cleanup.

The ignored `remote_native_interrupted_process_acceptance` uses `WUKONG_REMOTE_WORKER_PROCESS_FAULT_TEST`: managed-SSH `config`, expected `manifest_sha256`, one pinned trusted `probe` (`path`/`sha256`), `fault` (`preparation`, `host` or `service`) and new absolute `ready`, `resume`, `output` paths. Preparation publishes `ready` before start so the independent operator can retain an exclusive native-input handle; host/service publishes it after start so the operator can verify actual owned process identity and target readiness before termination. The updated probe uses `--fault-ready` to write a fixed marker in the current private scratch; the operator checks image hash, parent process and marker bytes, instead of inferring execution from coarse CPU counters or elapsed time. For the service case, the operator also retains a query-only guardian Job handle, forcing recovery to reopen and drain the actual object. Only after the action/recovery is independently observed does the operator create `resume`.

The optional fault-profile `launch_spec: true` uses the same probe and fault protocol with a manifest-owned `work/nested` cwd. The operator must validate that protected launch binding and observe the marker inside that cwd before injecting the fault. This additional service-interruption case also passed on that VM at `6e2bc91`, including independent query-only guardian observation and nested scratch cleanup.

The additional `fault: os_boot` profile requires `resource_inventory: true`. The independent operator first verifies the actual owned probe and protected request, records prior originals, and reboots only the dedicated VM. After independently checking cold qualification and original cleanup evidence, create `resume` within 360 seconds. This branch retains the old transport/binding, rejects an old start at reconnect, requires only the boot identity to change and lease epochs to increase, refuses old-run discovery/resource lookup and old input reuse, then admits and discovers a newly uploaded execution in a fresh run. Old-boot results are checked through independent original evidence, rather than weakening native run bindings to expose them. Compiler/library checks passed; this new controller boot-boundary branch is not yet VM-qualified.

The fixture performs no termination or task registration itself; it checks bound unknown-outcome cleanup, immutable recovery bytes, no start replay, stale service fences and successful admission afterward. Coordination files are signals, not OS evidence. The three concrete fault cases passed on the provided offline VM on 2026-11-13; see the source/content identities and limits in the [Windows qualification record](../windows/README.md). Earlier completed-result recovery does not cover these cases.

```powershell
$env:WUKONG_REMOTE_WORKER_PROCESS_FAULT_TEST = (Resolve-Path .\remote-process-fault-profile.json)
cargo test --locked --lib remote_native_interrupted_process_acceptance -- --ignored --nocapture
```

```powershell
$env:WUKONG_REMOTE_WORKER_PROCESS_TEST = (Resolve-Path .\remote-process-profile.json)
cargo test --locked --lib remote_installed_windows_worker_process_acceptance -- --ignored --nocapture
```

After those jobs close, independently restart only the dedicated test service and observe its new qualification receipt, preserving the old receipt and connection identity. `remote_native_completed_process_recovery_acceptance` then uses `WUKONG_REMOTE_WORKER_PROCESS_RECOVERY_TEST`: managed-SSH `config`, absolute `previous` report path, its `previous_sha256` and a new absolute `output` path. It checks the same OS/install/package identity and increasing lease epoch, retrieves both completed results under their original stable requests without re-upload, and compares original receipt bytes. The fixture performs no service restart, install or uninstall. Both fixtures passed on the provided offline VM on 2026-11-13 at worker source `fbf7912`; read-only SQLite inspection and independently copied originals corroborated six closed resources, completed effects and empty scratch. Interrupted-resource cleanup/recovery, preparation cancellation and the other execution/lifecycle paths remain unfinished.

```powershell
$env:WUKONG_REMOTE_WORKER_PROCESS_RECOVERY_TEST = (Resolve-Path .\remote-process-recovery-profile.json)
cargo test --locked --lib remote_native_completed_process_recovery_acceptance -- --ignored --nocapture
```

- `unit/`: portable unit/trusted-local fixture tests, with explicit platform skips.
- `integration/`: actual isolated worker/Linux/GDB/AFL++/Ghidra acceptance.
- `support/`: shared protocol clients, separate from production code.

Run `python workers/tests/run.py unit` from the repository; `-p 'test_workbench*.py'` selects a subset. Full Linux unit tests need a regular user: trusted fixtures use `trusted_local`, which production bridge refuses as root. The entry point configures imports; do not discover tests by scanning runtime directories or count importing live scripts as acceptance.

Images place runtime in `/opt/wukong/runtime` and tests in `/opt/wukong/tests`, preserving the production bridge entry. Start full unit tests as a regular container user:

```sh
docker exec --user 10001:10001 wukong-worker python3 /opt/wukong/tests/run.py unit
```

Inside a worker with isolation/capabilities configured, root runs isolation acceptance and tests requiring target-UID switching:

```sh
python3 /opt/wukong/tests/run.py unit -p 'test_workbench*.py'
python3 /opt/wukong/tests/run.py integration --state /state/acceptance --fuzz
python3 /opt/wukong/tests/run.py isolation /state/isolation-acceptance
python3 /opt/wukong/tests/run.py launch --state /state/launch-acceptance
python3 /opt/wukong/tests/run.py ghidra --state /state/ghidra-acceptance
```

Use directories owned exclusively by this test run. See [worker requirements](../runtime/README.md).

The `launch` suite builds a trusted C fixture and exercises both stdin and argv-file LaunchSpec jobs through configured Linux UID isolation. It checks a declared peer outside cwd, writable cwd output, raw binary stdout, immutable original files and stable starts. Run it with the documented capabilities, read-only container root and executable `/tmp`. Portable planner/unit checks do not replace this isolation acceptance.

The native Windows service-access fixture disables the copied administrator token's maximum privileges as well as its administrator group. This tests ordinary-account DACL rights without `SeTakeOwnershipPrivilege` granting `WRITE_OWNER`; it changes neither the parent token nor the installed service policy. The fixture passed on the provided Windows 10 VM on 2026-11-13.

### Managed native WinDbg launch acceptance

`src/runtime/ssh_debugger_acceptance.rs` exercises the installed service through the real controller transport. Set `WUKONG_REMOTE_WORKER_DEBUGGER_TEST` to a machine-local JSON profile containing the pinned managed-SSH `config`, `manifest_sha256`, new absolute `output`, and `probes` keyed by `x64`/`x86`. Each architecture supplies `program` and `pdb`, each with an absolute local `path` and exact `sha256` from that signed package. Run `cargo test --locked --lib remote_installed_windows_worker_debugger_acceptance -- --ignored --nocapture`. The fixture uploads only those sealed bytes and an offline LaunchSpec; it installs no software. RPC JSONL is fsynced after each call, and final raw MCP transcripts are retained alongside the report.

On 2026-11-13 the fixture passed in the dedicated offline Windows 10 VM at native source `5f2b692`, manifest `a8a755a2cd17c597f0ecfa87848db668aeba7654d6cddf4b5c90b45146a427a7`. Both architectures covered PDB symbol breakpoints, numeric enable/disable/removal, registers, memory, modules, stack, raw thread results, disassembly, step/next/finish, asynchronous continue/interrupt, immutable live transcript prefixes, blocked lease release, cached close and refused restart after closure. The 57 controller calls produced 60 MCP calls, including two expected non-fault exception refusals preserved as original errors. The first fixture attempt incorrectly expected exception information at an ordinary breakpoint; its target was closed and its original evidence independently collected before the corrected run. Read-only SQLite and independent VM checks found three closed resources, 32 completed effects, empty scratch and no remaining owned debugger/probe processes. All 95 original files were hash-verified, earlier running services and management SSH were preserved, and the ISO was detached. The report SHA256 is `cc50a5b0c35e00a21916bf8f8cab50731c20b8d9d451769d149b25db76ff0a04`; x64/x86 raw transcript hashes are `1934765a4e5dde8ff5465c0269393703aa73c43e5a6a075c2a188eefc1b6d58f` and `d825ff0644e0e5a538ea99f4889755354614a21c8d8d1796463f653d0f1b8986`. This qualifies the stated contained launch/control/close path, not complete image/PDB/dump admission, service interruption recovery, all adversarial boundaries or production availability.

The current fixture additionally exchanges the two architectures' PDB files in separately admitted LaunchSpecs. Each rejected start must retain exactly one resource, finish owned recovery and refuse replay before the valid case proceeds. The valid start/cleanup must retain the exact admitted PE/PDB hashes and matching engine metadata without claiming loaded PDB byte verification.

On 2026-11-13 this extension passed against a fresh offline installation of `4edc290`, manifest `f3a32f66d5fa3eb7330f4671826c84273ab01d80536f61ac7f1bd34cd284c062`. Its 75 controller calls retained 62 MCP calls (two expected non-fault exception refusals). Both swapped PDBs were refused; an independent read of protected originals confirmed the exact GUID/linker-age/machine mismatch error, absent target preparation intents and absent MCP transcripts, followed by removed scratch. Correct x64/x86 launches retained matching PE/PDB identities and passed the full control/close sequence. Read-only SQLite confirmed four closed resources and 28 settled effects. All 88 original files were hash-verified; the 14 previously running services and management SSH remained running, the VM had no default routes, and the ISO was detached. The report SHA256 is `b0e6d49949120cbf67d1006583f46c71f91e3bc22c9c5a2ebb194c9b127b8666`; x64/x86 transcript hashes are `4f5b2fc57c3e8d5535cfb6f11d546b6b71fa179b1a2eac54b1accc687b598e0d` and `2b89a5816d289b81561bbc6423b4da0259d6a7d86be9524d191cf49380a57b47`. This qualifies those metadata/negative fixtures, not loaded PDB byte identity, dumps, service interruption recovery or general adversarial admission. The cancellation fix is present in this build but its interruption scenario remains unqualified.

The fixture now also queries normal and recovered terminal states through `debug.inspect`, checks that recovery remains unknown and reads the original recovery receipt through `native-results/<id>/receipt`. These additional assertions require a newer installed worker; the `4edc290` result above does not cover them.

### Managed debugger interruption acceptance

`ssh_debugger_fault_acceptance.rs` reuses the sealed-probe uploader, RPC recorder, coordination files and post-recovery echo fixture. `WUKONG_REMOTE_WORKER_DEBUGGER_FAULT_TEST` selects a local profile with pinned `config`, `manifest_sha256`, one `probe` containing `program`/`pdb`, `fault` (`cancel`, `host`, `service_stop` or `service_kill`) and new absolute `ready`, `resume` and `output` paths. Run `cargo test --locked --lib remote_native_interrupted_debugger_acceptance -- --ignored --nocapture`. At the trusted probe's `wmain` breakpoint it removes the breakpoint and issues a fixed step-out that remains pending because this fixture loops forever. The independent VM operator must verify the original protected request, owned processes and pending journal mutation before allowing the selected fault. For cancel it publishes the resume marker after that observation; the other modes publish it after their independent fault action. `cancel` uses a second authenticated controller connection with the same lease because a single transport is serialized. The test expects an unknown original outcome, confirmed cleanup, hash-bound recovery evidence, refused start/control replay and successful fresh echo execution. Service restart must advance the lease epoch and reject the old fence. Build/Clippy checks do not qualify these real faults; no installed interruption result is claimed yet.

### External WinDbg MCP acceptance

`tests/windbg_mcp.rs` drives a real external debugger through the existing `McpRegistry` (Rig/rmcp). It is ignored by default and requires a dedicated VM with the repository's trusted x64/x86 `wukong-probe` executables, matching PDBs and generated dumps. Configure a pinned SSH stdio command or a local command inside that VM; this test does not install software or run unknown targets.

Set `WUKONG_WINDBG_MCP_ACCEPTANCE` to a machine-local JSON file with these fields:

- `server`: the existing `McpServerConfig` fields `name`, `command`, `args`, `env` and `timeout_seconds`. Use `name: "windbg"`, the verified `windbg-mcp.exe` with `--tools session,inspect,exec,crash`, and a 45-second timeout. For packaged Win32-OpenSSH, include `"PROGRAMDATA"` in the environment allowlist. SSH must pin the host key and disable forwarding.
- `fixtures`: two objects, in x64 then x86 order, each containing `architecture`, `target`, `dump` and `symbols`. Paths are absolute paths in the VM; `symbols` is the local directory holding that fixture's PDB.
- `transcript`: a new absolute path on the controller for the raw MCP JSONL evidence. Existing output is never overwritten.

```powershell
$env:WUKONG_WINDBG_MCP_ACCEPTANCE = (Resolve-Path .\windbg-mcp-acceptance.json).Path
cargo test --locked --test windbg_mcp existing_rig_client_drives_external_windbg_mcp -- --ignored --nocapture
```

On 2026-11-13 this passed on the provided offline Windows 10 VM with [glslang/windbg-mcp v0.24.0](https://github.com/glslang/windbg-mcp/releases/tag/v0.24.0), archive SHA256 `f6150397f7bc07a6d5fbd773adde3ec4b78639378abb5edec17be7201b1d3f34`, and the sealed Debugging Tools engine. It covered both architectures' dumps and live launches, local symbols, the `wmain` breakpoint, registers, 16 instruction bytes, modules, backtraces, disassembly, single stepping, release and an empty final session list: 51 MCP calls, no tool errors. WOW64's extra loader breakpoint is bounded explicitly. Thread-list text is retained as raw evidence; it is not claimed to be a structured thread inventory. These results establish client/backend interoperability, not managed AppContainer isolation, cross-task ownership, reboot recovery or complete replacement of the legacy adapter.

The separate `external_windbg_mcp_run_handles_are_bounded_and_fenced` fixture uses `WUKONG_WINDBG_MCP_RUN_ACCEPTANCE` with the same profile shape and a new transcript path; `dump` may be omitted. It launches only the sealed looping probes. It exercises upstream `continue_async`, `wait_for_stop` and `break_in`, including repeated stop reads, non-cancelling zero-duration polls, rejected stale run handles, repeated run-bound break-in and the engine's execution deadline. No replacement execution scheduler or console parser is involved.

```powershell
$env:WUKONG_WINDBG_MCP_RUN_ACCEPTANCE = (Resolve-Path .\windbg-mcp-runs.json).Path
cargo test --locked --test windbg_mcp external_windbg_mcp_run_handles_are_bounded_and_fenced -- --ignored --nocapture
```

On 2026-11-13 this asynchronous fixture passed on the dedicated offline Windows 10 VM against the installed `10a6860` package, manifest `fd9af35aa002ca8c9393f54f8bb03de872c1fd074c0e691cf7fe2b9edebb5636`, using pinned WinDbg MCP 0.24.0 with the documented closed-stdin-pipe patch. Both x64 and x86 covered the assertions above. The 35-call transcript contains 33 successes and two expected stale-handle refusals; SHA256 is `f7709ce5aaa9722a90c74e6127b2fed61a39392f052836dde2078e21f1196c2f`. Both sessions reported released/worker-terminated without leaving a running target, and the final session list was empty. Independent before/after checks verified the tool and both EXE/PDB pairs, no remaining owned tool/target processes, running worker/management services and no VM default routes. This was direct Rig/rmcp interoperability using trusted fixtures, not managed native session admission or AppContainer acceptance. No installation or service restart was needed.

Bootstrap PE format fixtures run directly through `object` in the standalone Rust library's `pe_imports` tests, covering normal/delay imports, both architectures, malformed directories and unsafe library names. Python automation separately checks the inspector's response binding, CRT refusal, timeouts and pre-execution hash verification; mocked observations are not PE format acceptance. The native format tests passed on the provided offline Windows 10 VM on 2026-11-13, and the real controller utility reproduced all three bootstrap and both compiled-probe import inventories locally.

`integration --static-fixture` exercises the debugger/replay adapter with a statically linked fixture; it does not validate dynamic loader/library identity. Docker execution needs executable `/tmp`, ptrace and the documented isolation capabilities.

The ignored Rust fixture `remote_windows_worker_reboot_binding_acceptance` checks a real dedicated Windows VM reboot while keeping the controller's old transport alive. First run the installed-worker controller acceptance described in the [Windows guide](../windows/README.md), so its immutable readback fixture exists and its leases are released. Set `WUKONG_REMOTE_WORKER_REBOOT_TEST` to a JSON profile containing the same pinned managed-SSH `config`, expected `manifest_sha256` and new absolute `ready`, `resume`, `output` paths; all three paths must initially be absent.

```powershell
$env:WUKONG_REMOTE_WORKER_REBOOT_TEST = (Resolve-Path .\remote-reboot-profile.json)
cargo test --locked --lib remote_windows_worker_reboot_binding_acceptance -- --ignored --nocapture
```

The test creates `ready` after authenticating and reserving a lease, then waits at most six minutes. The operator reboots only the dedicated VM through its independent pinned management connection, verifies automatic service readiness and a new schema4 qualification receipt, then creates `resume`. The test itself performs no reboot, installation or uninstall. It rejects the old controller binding before a write, checks the changed OS boot identity and increasing durable epoch, and uses a fresh leased connection to verify the existing readback fixture and absent write sentinel. `output` contains identity/counter observations without lease tokens or key contents. The provided offline Windows 10 VM passed on 2026-11-13 at worker source `7b6c4c0`, including automatic management SSH/native-service startup; original receipts and transcripts were separately hash-verified. This covers fixed trusted resources and reboot binding, while general analysis-resource recovery and the complete offline lifecycle remain unfinished.
