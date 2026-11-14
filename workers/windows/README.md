# Native Windows worker components

English · [简体中文](README.zh-CN.md)

## Ownership and request flow

The native Rust worker is the sole Windows execution authority. Managed SSH invokes the fixed relay, which authenticates the enrolled SCM service through its named pipe. The service owns startup, qualification, recovery and shutdown; `worker.rs` routes the public API. There is no persistent Python metadata worker, inner Python lease, loopback worker endpoint or reverse broker-health channel.

| Component | Responsibility |
| --- | --- |
| Controller / Rig | Agent scheduling, roles, durable business requests, evidence and independent verification |
| `workers/protocol` | Shared identity, lease, error and capacity types; public JSON-RPC semantics |
| `worker_catalog.rs` | One method/capability registry for routing and discovery |
| `worker_identity.rs`, `worker_leases.rs` | OS boot binding and exclusive controller/run fencing |
| `inputs.rs`, `inputs_transfers.rs` | Immutable uploads, resumable offsets, owner-scoped reads and execution-input proofs |
| `resource_runtime*`, `resource_journal.rs` | Jobs/sessions, durable side effects, observation and recovery |
| Resource/session hosts | Owned Windows processes, AppContainers, Jobs, ACL/WFP boundaries and cleanup |
| External WinDbg MCP / Python | Debugging or bounded analysis inside owned resources; no independent worker authority |

The public transport remains JSON-RPC 2.0. The controller uses `jsonrpsee`; shared Rust types use Serde. Private session messages use generated Prost protobuf and Tokio framing; the external debugger uses `rmcp`. Raw debugger replies remain hashed evidence instead of being reimplemented as a new debugger protocol. Linux retains its existing Python worker.

`ping` and `doctor` are answered natively. The service enables registered execution capabilities only after signed-package checks, outstanding-resource recovery, offline dependency checks and a fresh isolation/debugger qualification for this service start. `doctor.readiness` explains this state. Historical qualification receipts cannot qualify a new startup. The registry advertises only callable native execution or isolated analysis methods; Linux GDB commands, AFL/QEMU and runtime preparation are not advertised on Windows.

## Fencing, capacity and durable state

The native worker owns one 120-second run/controller lease. The controller renews it over a separate transport every 15 seconds. Protected `metadata/lease-epochs/` records reserve monotonically increasing epochs before issuance, with an exclusive writer handle and a 100,000-epoch bound. Renewal preserves the fence. Identity and lease checks precede cached-effect access. Expiry requests cancellation and does not prove resource cleanup; active requests/resources prevent takeover or release.

`doctor.capacity.max_active_resources` is currently **1**, shared by jobs, persistent sessions and analysis helpers. The SQLite uniqueness constraint enforces this capacity. An occupied worker returns `resource_busy` before admitting a new effect. The controller uses a FIFO Tokio admission mutex and bounded retries; observation, close, cancellation and lease renewal remain available while a start waits. A live session must be closed before another resource can start. Transport failures and unknown outcomes are never automatically replayed. The controller business ledger and worker resource journal retain distinct responsibilities.

Uploads are stored once in protected native storage. `file.put` accepts at most 32 MiB; resumable uploads use 1-MiB chunks and accept at most 16 GiB. Upload declarations, content SHA256, executable policy and published receipts bind the worker identity, run, controller and lease epoch. Duplicate chunks must match original bytes; status reports the native durable offset. Reads and execution require completed receipts. Inputs are copied into owned resource scratch only when a tool needs them; no ordinary-account file receipt can authorize System reads. Original attachments remain unchanged.

## Execution and analysis

`job.*`, `debug.*`, `interact.*`, `exec.run`, `py.run`, replay, minimization and verification use the same native resource lifecycle and journal. A repeated request resolves the original intent before accessing successor inputs. Cleanup failures retain ownership; interrupted execution remains unknown unless original evidence proves an outcome. Artifact access is run-scoped and retains size/SHA256 identity. Resource results describe their isolation; only `doctor` reports current service readiness.

`runtime.profile` reads admitted content identities directly from protected native storage without occupying an execution slot. `debug.profile` and `interact.profile` observe only the session-owned process handle, retain the original evidence and return bounded previews. Unavailable observations remain unknown; mapped paths do not prove file hashes. Static binary declarations remain controller-owned.

`seccomp.analyze` invokes the packaged analysis code through a disposable AppContainer Python process. The helper consumes only admitted inputs, has a 120-second deadline and an 8-MiB output limit, and retains execution receipts/stdout hashes. It neither constructs the Python worker runtime nor owns leases, daemons, sessions or a second job ledger.

The current offline package does not ship pwntools, pyelftools or Unicorn, so cyclic-offset, libc-identification and emulation methods are not registered or advertised. Adding a capability requires its offline dependencies and callable-path acceptance.

### Native Python source jobs

`py.run` launches packaged CPython with fixed `-I -B -u -X utf8` options. Source is nonempty UTF-8, at most 256 KiB without NUL, stored as a sealed non-executable input. It permits at most 64 argv entries and an optional admitted target, a 10–3600-second timeout and 1-KiB–8-MiB output. The existing 16-process/1-GiB Job limits apply. Source never executes in the System service. Python syntax failures produce a failed job and original stderr; retries retain the original source-bound intent.

## Offline delivery and validation

Build in an x64 MSVC environment. Keep model credentials and signing private keys on the controller. Install/uninstall acceptance belongs only on dedicated VMs or isolated runners, never over a developer's installation.

```powershell
python -m scripts.worker.toolchain --cache target/windows-offline-cache --download
python -m scripts.worker.build_native
python -m scripts.worker.layout --cache target/windows-offline-cache --output target/windows-offline-layout
python -m scripts.worker.assemble --cache target/windows-offline-cache --output target/windows-worker-package --keys ABSOLUTE_CONTROLLER_KEY_DIRECTORY
cargo fmt --manifest-path workers/windows/Cargo.toml --all -- --check
cargo clippy --locked --manifest-path workers/windows/Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path workers/windows/Cargo.toml --all-targets
```

Native build identity includes the shared protocol crate, native/Python sources, locked inputs and build scripts. Assembly verifies this identity and preserves dependency licenses. Enrollment pins the controller signer and installation; SSH independently pins the worker host key. Use the [cross-platform environment guide](../../docs/design/cross-platform-environments.md) for connection and task binding, and [worker acceptance guide](../tests/README.md) for installed process/debugger/recovery fixtures.

Unit tests establish contract and ownership invariants; they do not establish VM isolation or live-model effectiveness. Installed acceptance must match the exact signed source/package and include the normal Wukong discovery/dispatch path. Existing [historical records](VALIDATION-HISTORY.md) preserve prior commits, hashes, failures and limits, including unfinished maintenance and adversarial acceptance. They do not qualify the current refactor. Kernel debugging, automatic VM creation/snapshots and GUI target interaction remain outside this worker contract.
