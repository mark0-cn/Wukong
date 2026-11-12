<a id="worker-测试"></a>

# Worker tests

English · [简体中文](README.zh-CN.md)

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
python3 /opt/wukong/tests/run.py ghidra --state /state/ghidra-acceptance
```

Use directories owned exclusively by this test run. See [worker requirements](../runtime/README.md).

The native Windows service-access fixture disables the copied administrator token's maximum privileges as well as its administrator group. This tests ordinary-account DACL rights without `SeTakeOwnershipPrivilege` granting `WRITE_OWNER`; it changes neither the parent token nor the installed service policy. The fixture passed on the provided Windows 10 VM on 2026-11-13.

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
