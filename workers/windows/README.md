# Native Windows worker components

English · [简体中文](README.zh-CN.md)

Windows execution requires a dedicated, disposable target VM. The target may affect the entire guest, including its worker. Configure host access, shared folders/devices and external networking at the hypervisor boundary; keep controller/model credentials and signing keys outside the guest. Retain original evidence on the controller and reset or discard the VM after untrusted execution. The worker does not create VMs, configure hypervisor policy or automate snapshots.

Signed enrollment must explicitly contain `"execution_boundary": "disposable_vm"`. The signing CLI, offline media preparation, installer and service enforce this declaration. Older signed records remain readable for historical identity checks but cannot authorize new startup under this boundary. Deploy a fresh disposable VM with a newly signed declaration; preserve earlier installations and evidence separately.

Startup records deployment identity, resource recovery, dependency self-test, guest controls self-test and communication readiness as separate stages. Each stage publishes start and success/failure events with elapsed time and a stable error code. Protected `metadata/startup-diagnostics.json` uses schema 2, is atomically replaced, capped at 16 KiB, and bound to the installation, verified package/boot identity when available, and a fresh startup attempt. `last-service-error.txt` is refreshed for the same attempt, including pending/ready states. Fixed summaries omit arbitrary exception text. Installer failures include current diagnostic identity and code; hosted acceptance validates the full report separately from installation success.

The project no longer subscribes to or matches WFP events, and startup does not require loopback-block evidence. AppContainer/token, ACL, process ownership, cleanup and debugger checks remain guest controls. Fresh schema-5 qualification receipts use `scope: "disposable_vm_guest_controls"`. Reports and `doctor` declare `execution_boundary: "disposable_vm"` and `external_isolation_verified: false`; `doctor.guest_controls_verified` describes the current startup checks. These checks cannot certify hypervisor isolation or evidence integrity after guest compromise. Windows may still use its own WFP facilities internally; this change does not disable the OS firewall.


## Ownership and request flow

The native Rust worker is the sole Windows execution authority. Managed SSH invokes the fixed relay, which authenticates the enrolled SCM service through its named pipe. The service owns startup, qualification, recovery and shutdown; `native/rpc/worker.rs` routes the public API. There is no persistent Python metadata worker, inner Python lease, loopback worker endpoint or reverse broker-health channel.

| Component | Responsibility |
| --- | --- |
| Controller / Rig | Agent scheduling, roles, durable business requests, evidence and independent verification |
| `workers/protocol` | Shared identity, lease, error and capacity types; public JSON-RPC semantics |
| `native/core/` | Ownership identity and immutable resource types, independent of RPC, storage and OS APIs |
| `native/host/` | Installation, SCM lifecycle, verified runtime context and qualification |
| `native/rpc/` | JSON-RPC registration, transport, boot binding and controller/run fencing |
| `native/resources/` | One resource manager owns admission, operation tickets, journal and recovery |
| `native/executors/` | Process/session plans, owned execution observations and result adapters; no journal mutation |
| `native/artifacts/` | Lease-scoped immutable inputs, run/resource-scoped outputs and bounded protected IO |
| `native/isolation/` | Windows tokens, AppContainers, Jobs, ACL and process launch |
| External WinDbg MCP / Python | Debugging or bounded analysis inside owned resources; no independent worker authority |

RPC resolves registered handlers into resource operations before calling the manager; resources and artifacts never query the RPC registry. `executors/results.rs` validates and publishes process/replay observations, while the manager alone commits cleanup and journal transitions. Shell and analysis adapters own their response projection. The shared protocol provides typed outcome/cleanup observations; the native journal retains its stricter OS cleanup proof.

The public transport remains JSON-RPC 2.0. The controller uses `jsonrpsee`; shared Rust types use Serde. Private session messages use generated Prost protobuf and Tokio framing; the external debugger uses `rmcp`. Raw debugger replies remain hashed evidence instead of being reimplemented as a new debugger protocol. Linux retains its existing Python worker.

`ping` and `doctor` are answered natively. The service enables registered execution capabilities only after signed-package checks, outstanding-resource recovery, offline dependency checks and a fresh guest controls/debugger qualification for this service start. `doctor.readiness` explains this state. Historical qualification receipts cannot qualify a new startup. The registry advertises only callable native execution or isolated analysis methods; Linux GDB commands, AFL/QEMU and runtime preparation are not advertised on Windows.

## Fencing, capacity and durable state

The native worker owns one 120-second run/controller lease. The controller renews it over a separate transport every 15 seconds. Protected `metadata/lease-epochs/` records reserve monotonically increasing epochs before issuance, with an exclusive writer handle and a 100,000-epoch bound. Renewal preserves the fence. Identity and lease checks precede cached-effect access. Expiry requests cancellation and does not prove resource cleanup; active requests/resources prevent takeover or release.

`doctor.capacity.max_active_resources` is currently **1**, shared by jobs, persistent sessions and analysis helpers. The SQLite uniqueness constraint enforces this capacity. An occupied worker returns `resource_busy` before admitting a new effect. The controller uses a FIFO Tokio admission mutex and bounded retries; observation, close, cancellation and lease renewal remain available while a start waits. A live session must be closed before another resource can start. Transport failures and unknown outcomes are never automatically replayed. The controller business ledger and worker resource journal retain distinct responsibilities.

The resource journal uses schema **2** and rejects older databases without migration or deletion. Use a fresh Worker installation for protocol v3 and retain earlier installations/evidence. Explicit lifecycle variants prevent a process, session and recovery operation from simultaneously occupying the single slot. Input preparation runs outside the resource lock and retains its separate input lock; admission then revalidates the lease and capacity. The manager reserves and issues a ticket, starts the owned Tokio task outside its lock, and commits the original observation against that ticket. Startup, process draining and recovery waits retain ownership independently of the calling RPC. Queries and cancellation can inspect an unfinished start or failed cleanup; failure never releases the journal blocker.

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

Unit tests establish contract and ownership invariants; they do not establish VM isolation or live-model effectiveness. Installed acceptance must match the exact signed source/package and include the normal Wukong discovery/dispatch path. The [2026-11-15 v3 acceptance record](../../docs/history/validation/2026-11-15-execution-contract-v3.md) binds the fresh installation, x64/x86 execution/debugging, restart recovery and complete Wukong workflow to their exact source/package and original evidence. Earlier [historical records](VALIDATION-HISTORY.md) retain their own commits, failures and limits, including unfinished maintenance and adversarial acceptance; those limits are not superseded by normal-path acceptance. Kernel debugging, automatic VM creation/snapshots and GUI target interaction remain outside this worker contract.

Windows Python sources are selected by `python-components.json`: the independent [analysis package](../analysis/README.md) and `python/selftest.py`. Helpers need only CPython standard libraries; `python/requirements.txt` is separate from Linux bridge dependencies. Offline packaging neither installs nor copies Linux worker modules. The signed source identity includes the analysis package and component inventory.
