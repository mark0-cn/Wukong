<a id="worker-测试"></a>

# Worker tests

English · [简体中文](README.zh-CN.md)

`remote_installed_windows_worker_launch_acceptance` uses `WUKONG_REMOTE_WORKER_LAUNCH_TEST`: managed-SSH `config`, expected `manifest_sha256`, a new absolute `output`, and `probes` with exactly `x64`/`x86`. Each architecture supplies `program` and `dependency`, each with absolute `path` and pinned `sha256`, from the package's `runtime/selftest/` directory. The trusted probe loads its sibling `wukong-fixture.dll`, reads a declared peer outside cwd, writes a new cwd file and copies binary stdin/argv input to stdout. The controller checks six resources, changed manifest/per-call override refusal, full request/receipt identity, cleanup and stable completed starts. No compiler or download runs in the VM. After an independently observed service restart, `remote_native_launch_recovery_acceptance` uses `WUKONG_REMOTE_WORKER_LAUNCH_RECOVERY_TEST` with `config`, absolute `previous`, its `previous_sha256` and a new absolute `output`; it compares four completed LaunchSpec results and original receipts without uploads or execution. Installed-VM qualification for these fixtures is pending.

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

The optional fault-profile `launch_spec: true` uses the same probe and fault protocol with a manifest-owned `work/nested` cwd. The operator must validate that protected launch binding and observe the marker inside that cwd before injecting the fault. This additional installed-VM case remains pending.

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

Bootstrap PE format fixtures run directly through `object` in the standalone Rust library's `pe_imports` tests, covering normal/delay imports, both architectures, malformed directories and unsafe library names. Python automation separately checks the inspector's response binding, CRT refusal, timeouts and pre-execution hash verification; mocked observations are not PE format acceptance. The native format tests passed on the provided offline Windows 10 VM on 2026-11-13, and the real controller utility reproduced all three bootstrap and both compiled-probe import inventories locally.

`integration --static-fixture` exercises the debugger/replay adapter with a statically linked fixture; it does not validate dynamic loader/library identity. Docker execution needs executable `/tmp`, ptrace and the documented isolation capabilities.

On Windows with MSVC and an existing Debugging Tools installation, build `cargo build --locked --bin wukong-dbgeng`, then run:

```powershell
python workers/tests/run.py windows-debugger --state target/dbgeng-acceptance --helper target/debug/wukong-dbgeng.exe --engine-dir '<Debugging Tools x64 directory>'
```

This compiles trusted x64/x86 fixtures and checks Dump registers, stack, memory, modules, threads, disassembly, live breakpoints, stepping, finish, request deduplication and deadline interruption followed by inspection. It retains hashes and raw transcripts in the selected directory. It neither installs tools nor proves production isolation or offline VM installation; those require dedicated VM acceptance.

For a compiler-free dedicated VM, copy only your self-built `fixture_x64/` and `fixture_x86/` directories from the printed attempt directory, then supply their parent with `--prebuilt-fixtures ABSOLUTE_DIRECTORY`. The VM still needs the complete offline Python/dependency and Debugging Tools payload; no VM package download is required. This entry also exercises actual `object`/DbgHelp/DbgEng format readers with positive PE identities and malformed PE/MSF/minidump inputs. It rejects conflicting PE timestamps, a valid PDB for another image, an image renamed to `.pdb`, and a dump whose known fixture RSDS GUID was changed while retaining the original matching PDB cache. It verifies that a missing cache cannot load the producer's external PDB. Portable unit tests separately cover correspondence policy and missing-identity gaps; their supplied observations do not replace native SDK acceptance. A file-index request after DbgEng initialization is rejected, and failed initialization ends the helper. This suite passed on the provided offline Windows 10 VM on 2026-11-13; its trusted sessions do not qualify AppContainer PDB loading, legacy full-user dumps, valid kernel-dump rejection or reboot recovery.

The ignored Rust fixture `remote_windows_worker_reboot_binding_acceptance` checks a real dedicated Windows VM reboot while keeping the controller's old transport alive. First run the installed-worker controller acceptance described in the [Windows guide](../windows/README.md), so its immutable readback fixture exists and its leases are released. Set `WUKONG_REMOTE_WORKER_REBOOT_TEST` to a JSON profile containing the same pinned managed-SSH `config`, expected `manifest_sha256` and new absolute `ready`, `resume`, `output` paths; all three paths must initially be absent.

```powershell
$env:WUKONG_REMOTE_WORKER_REBOOT_TEST = (Resolve-Path .\remote-reboot-profile.json)
cargo test --locked --lib remote_windows_worker_reboot_binding_acceptance -- --ignored --nocapture
```

The test creates `ready` after authenticating and reserving a lease, then waits at most six minutes. The operator reboots only the dedicated VM through its independent pinned management connection, verifies automatic service readiness and a new schema4 qualification receipt, then creates `resume`. The test itself performs no reboot, installation or uninstall. It rejects the old controller binding before a write, checks the changed OS boot identity and increasing durable epoch, and uses a fresh leased connection to verify the existing readback fixture and absent write sentinel. `output` contains identity/counter observations without lease tokens or key contents. The provided offline Windows 10 VM passed on 2026-11-13 at worker source `7b6c4c0`, including automatic management SSH/native-service startup; original receipts and transcripts were separately hash-verified. This covers fixed trusted resources and reboot binding, while general analysis-resource recovery and the complete offline lifecycle remain unfinished.
