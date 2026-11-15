<a id="执行-worker"></a>

# Execution worker

English · [简体中文](README.zh-CN.md)

The Python process manager in this guide serves Linux workers and trusted development fixtures. Managed Windows uses one native worker for identity, capabilities, leases, uploads and execution; Python runs isolated analysis on demand. See the [Windows component guide](../windows/README.md) for current ownership and capacity limits.

The Rust controller never runs an analysed target by default. Choose a persistent
Docker/VM/SSH worker. `trusted_local` explicitly runs trusted fixtures on the host
and is only suitable for development tests. The Python bridge manages processes;
the container or VM supplies isolation.

For Linux workers and trusted development fixtures, install bridge dependencies in the Python environment used by the worker
(or `environments.<id>.runtime.python` for trusted local tests):

```sh
python -m pip install -e workers/analysis -r workers/runtime/requirements.txt
```

Managed Windows VMs require the complete offline package, including Python and all dependencies; do not fetch packages or run online pip installation in the VM. The [package contract](../package/README.md) and [Windows components](../windows/README.md) describe the implementation and source-bound installation/production acceptance limits.

Bridge startup disables Python bytecode writes before importing worker modules, including in new daemon/job-supervisor processes. This keeps signed deployment files unchanged; writable state remains in the configured state directory. Launch other Python entry points from an immutable deployment with `-B` as well.

Metadata publication uses `atomicwrites`: complete UTF-8 JSON is flushed before replacement, and failed writes clean up their temporary files. The library handles Windows write-through moves and POSIX parent-directory synchronization. Only transient Windows access/sharing violations retry, at most five times; a failed reservation never authorizes execution. Corrupted existing operation records remain errors and are never silently reset. Linux bridge dependencies are pinned in `requirements.txt`; native Windows metadata does not use this Python implementation. Failure-injection checks do not establish power-loss or current-source VM reboot acceptance.

Job admission builds a typed internal plan before reserving resources. Network/LaunchSpec/replay/process validation normalizes a private parameter copy and computes the existing request fingerprint; the job manager retains one lock across duplicate lookup, reservation, UID allocation and supervisor creation. A failed spawn leaves its reservation for reconciliation and never authorizes an automatic respawn. Wire schemas and state formats remain owned by the shared protocol and existing ledgers.

Child environment policy, RPC error categories and atomic metadata IO have separate owners. Consumers import them directly; numerical/address validation comes from `wukong_analysis.validation`, and protocol definitions come from the generated contract. Artifact helpers retain path confinement, hashes and bounded reads. Metadata retries, error categories and child environment filtering keep their existing behavior.

Session recovery reports unreadable JSON or IO failures on stderr, with at most 16 detailed warnings per session kind and one count of additional failures. Diagnostics contain only the kind, a recognized session ID and exception class; record contents and arbitrary paths are excluded. Damaged records stay unchanged, former running owners become lost when their metadata can be written, and missing required state fields still fail startup. RPC stdout remains reserved for protocol responses.

The Docker image installs these automatically. GDB/MI results include a parsed
`payload`; asynchronous events are structured records with type, message, payload
and the original `record`. Original MI transcripts remain available as evidence.

Isolated shell targets, Python targets and process inputs share the execution-copy preparation path. `py_run.target` is appended as a read-only executable copy, not as data requiring chmod. Raw `job_start` can name up to sixteen staged argv files in `executable_inputs`; other argv files stay read-only data. LaunchSpec retains authority over its executable files, cwd and input mode. Results expose execution cwd and each prepared file's source hash, executed hash and access mode. Copy hash mismatches fail preparation; original attachments are never chmodded. Each isolated invocation has a fresh writable working directory, so retrieve artifacts explicitly instead of assuming another call sees relative files.

## Docker

From the repository root (PowerShell):

```powershell
python -m scripts.toolchain.fetch_worker_deps
$docker = 'C:\Program Files\Docker\Docker\resources\bin\docker.exe'
$env:PATH = (Split-Path $docker) + ';' + $env:PATH
& $docker build -f workers/Dockerfile -t wukong-worker:0.1 .
& $docker volume create wukong-worker-state
& $docker run -d --init --name wukong-worker --network none --cap-drop ALL `
  --cap-add CHOWN --cap-add SETUID --cap-add SETGID --cap-add KILL --cap-add SYS_PTRACE --cap-add DAC_OVERRIDE `
  --security-opt no-new-privileges --security-opt seccomp=unconfined `
  --pids-limit 256 --memory 4g --cpus 2 --read-only --tmpfs /tmp:rw,size=1g,exec `
  --mount type=volume,source=wukong-worker-state,target=/state wukong-worker:0.1
```

Keep the container running to preserve sessions. The controller uses `docker exec`
stdio without publishing a network port. Files are uploaded into the named volume;
the host filesystem and model credentials are not mounted. Add a stronger VM
boundary for kernel experiments. Do not attach the Docker socket to the worker.

Use Docker `--init` (or a VM init) to reap orphaned descendants, including Ghidra helpers and detached worker daemons. Workbench creates containers with this option and refuses to reuse older containers without it; close active work, then use `env update --image <image-or-ID>` for the main worker, or `challenge down <id>` followed by `challenge up <id>` for a challenge worker. Challenge recreation retains files and named tool-data volumes; `env update` alone does not recreate existing challenge workers. `ProcessResources` owns execution and session process groups and closes them on success, timeout, cancellation and exceptions. Durable job supervisors are waited by their daemon without cancelling work when a client disconnects. Shutdown rejects concurrent requests or nonterminal jobs, closes sessions and waits for supervisor exit. The registry never calls `waitpid(-1)` or performs global process cleanup.

The Dockerfile pins the AFL++ base image by digest and installs GDB, GCC, Python,
AFL++ with QEMU instrumentation, the Python analysis toolchain (pwntools,
z3-solver, angr, ropper), Temurin JDK 25 LTS (checked SHA256; required by
Ghidra 12.2), and the official Ghidra 12.2 release with a checked SHA256. Use
`--build-arg INSTALL_GHIDRA=0` to omit Ghidra. To pin the base image, set `AFL_IMAGE`
to an upstream image digest after reviewing `docker image inspect` output. Full reproducibility remains limited by incompletely pinned transitive dependencies.

Register a named environment in the application configuration:

```toml
default_environment = "linux"
[environments.linux]
name = "Linux analysis"
platform = "linux"
[environments.linux.runtime]
backend = "docker"
docker_container = "wukong-worker"
# Optional: auto-detected on Windows Docker Desktop, otherwise "docker".
# docker_executable = 'C:\Program Files\Docker\Docker\resources\bin\docker.exe'
remote_bridge_path = "/opt/wukong/runtime/bridge.py"
remote_state_dir = "/state"
request_timeout_secs = 300
```

<a id="ssh-或-vm"></a>

## SSH or a VM

Install Python 3.12+, GDB and AFL++ with `afl-qemu-trace` or `afl-qemu-bridge` on an
isolated Linux guest. Install `workers/analysis` into its Python environment, copy this directory to `/opt/wukong/runtime` and the generated `workers/protocol/schema` to `/opt/wukong/protocol/schema`; configure a
dedicated management account inside that isolated guest. Production mode requires
a root-owned daemon so it can allocate a distinct unprivileged UID per job/session
and keep supervisor metadata private. The current SSH transport does not insert
sudo automatically; use a suitably configured isolated guest account. Key based
SSH must already work; the transport uses `BatchMode=yes`.

```toml
default_environment = "linux"
[environments.linux]
name = "Linux VM"
platform = "linux"
[environments.linux.runtime]
backend = "ssh"
ssh_host = "root@analysis-vm"
remote_bridge_path = "/opt/wukong/runtime/bridge.py"
remote_state_dir = "/state/wukong"
```

Model credentials are removed from child environments. Job environment overrides
are restricted to AFL/QEMU and sanitizer settings. State directories must not be
shared between mutually untrusted users.

Execution, replay, debugger and launch-description inputs use the public-artifact path policy. Isolated file/directory staging cannot publish transport endpoints, job descriptors or other private worker metadata as program inputs. Uploaded payloads under `files/` can use names such as `status.json`; confinement is checked after resolving aliases. Directory staging validates all members before copying any contents.

The internal launch resolver can stage its prepared runtime derivative outside the state directory only after checking the fixed cache path, ownership/permissions and exact executed-file hash. This exception is not an RPC option and cannot authorize metadata inputs; original and executed target identities remain separate.

In isolated GDB sessions, raw MI commands pass an operation-specific argument grammar before reaching the privileged debugger. Expressions support simple registers/symbols, dereferences, constant indexes and arithmetic/bitwise/comparison operators. Function calls (including host convenience functions), parentheses/casts, assignments and increments are rejected. MI strings use supported JSON escapes and are reserialized; octal/hex escapes and unchecked options are rejected. Dump inspection applies the same argument checks in addition to its read-only operation allowlist. Prefer typed `debug.inspect` / `debug.control` for structured requests; this policy does not make GDB itself a security sandbox.

Production Linux metadata is owned by the trusted daemon; target/fuzz processes
drop supplementary groups and run under separate allocated UIDs. Each experiment
has read-only staged files and its own writable directory. GDB keeps its trusted
owner and launches the inferior with the experiment UID. CHOWN/SETUID/SETGID are
needed for this separation; KILL lets the trusted supervisor cancel process groups owned by experiment UIDs. SYS_PTRACE and DAC_OVERRIDE are needed by the trusted
debugger to read/write inferior memory through procfs. These capabilities are not
retained by target processes. The read-only container filesystem, private state
volume, disabled network and VM/container boundary remain separate controls.
`trusted_local` explicitly reports a weaker, trusted-fixture boundary.

<a id="协议"></a>

## Protocol

JSON-RPC 2.0 requests and responses are one JSON object per stdio line. Error
responses identify unavailable tools; `doctor` returns actual capabilities,
executable paths and versions instead of claiming missing backends are usable.
Requests/responses are capped at 48 MiB. Single-frame file uploads are capped at 32 MiB; resumable transfers support up to 16 GiB using chunks of at most 1 MiB. See [analysis environments](../../docs/design/cross-platform-environments.md).
The controller uses jsonrpsee over bounded stdio frames. RPC errors preserve a
healthy connection; malformed/oversized frames and timeouts close it. The next
call reconnects, without automatically replaying the failed request.

Relay-managed daemon startup uses the existing `filelock` dependency and a kernel-backed `startup.guard` lock. The lock is held through readiness; competing relays recheck the endpoint after acquiring it. The daemon independently holds `daemon.guard` throughout initialization and service, so a crashed starter cannot authorize a competing daemon. Process exit releases ownership without deleting a timestamp-based stale lock. All relays sharing a state directory must use the same worker deployment. Startup allows 30 seconds for initialization, including the Windows boot-identity query. Readiness probes have a separate one-second response timeout; a child that exits before readiness reports its exit status and at most the final 2048 bytes of the local `daemon.log`.

| Method | Parameters / result |
| --- | --- |
| `doctor` | Actual worker platform, isolation label, core handler and capabilities/tool paths |
| `file.put` | `path`, `data_base64`, `executable`; immutable `files/<path>` artifact |
| `file.get` | `path`, optional `max_bytes`; base64 artifact and SHA256 |
| `job.start` | `argv`, optional `cwd`, `timeout_secs`, `output_limit`, `env`; `job_id` |
| `job.status`, `job.list` | Durable job metadata and bounded output summaries |
| `job.cancel`, `fuzz.stop` | `job_id`; cancel process tree |
| `job.artifacts` | `job_id`; content hashes and worker-relative paths |
| `fuzz.start` | `target`, `seeds`, `args`, `timeout_secs`, `exec_timeout_ms`, `memory_mb`; AFL++ `-Q` job |
| `fuzz.status` | Job state, parsed `fuzzer_stats`, hashed crash artifacts |
| `debug.start` | `target`, `args`; a worker-owned `session_id` |
| `debug.command` | `session_id`, one GDB/MI `command`, optional `timeout_secs` |
| `debug.events`, `debug.close` | `session_id`; bounded async records / stop session |
| `interact.start` | `argv`, optional `env`; a persistent interactive process with a worker-owned `session_id` |
| `interact.write` | `session_id`, `data_base64`, optional `read_timeout_secs`; writes stdin and returns newly produced bounded output. After leader exit, waits for stdout EOF up to the read deadline; `timed_out` can remain true while final pipe output is pending, and `data_sent=false` means no new write was attempted. |
| `interact.close` | `session_id`; terminate the session's process tree |
| `angr.solve` | `target`, `find`, optional `avoid`/`stdin_length`/`max_steps`/`timeout_secs`/`printable_only`; a bounded symbolic-execution job whose result JSON carries `input_base64` and exploration stats |
| `crash.replay` | `target`, `input`, `args` (`@@` input substitution), timeout; job |
| `input.minimize` | Replay parameters, `max_attempts`, `minimize_timeout_secs`; delta debugging job |
| `poc.verify` | Replay parameters, `trials`, distinct `producer_agent_id` / `verifier_agent_id`, `call_id`; job |
| `ghidra.query` | `target`, `target_sha256`, typed `query`, timeout; Ghidra results and hashed worker artifacts |
| `worker.shutdown` | Rejects active jobs, closes debug sessions, shuts down daemon |

Symbolic jobs use angr's native `Explorer` through `explore(n=max_steps)`, including initial-state matching and avoided-address handling. The result reports observed steps; the independent job timeout also bounds execution.

Mutating requests should include the controller's stable `request_id`. The durable
operation ledger records a reservation before effects and the result afterwards.
Repeating the same ID with changed parameters fails. Job IDs are deterministic
when `request_id` is supplied. If the debugger owner dies mid-command, the outcome
remains unknown and that command is never blindly replayed.

Job states are `queued`, `running`, `completed`, `failed`, `cancelled`, `timed_out`,
and `lost`. Closing a controller transport does not cancel work. A detached
supervisor drains output, enforces per-stream byte caps, handles cancellation and
updates a token-bound heartbeat. An expired heartbeat triggers a process creation
identity check (Windows creation FILETIME or Linux `/proc` start time). Only an
absent/changed supervisor is marked `lost`; uncertain observations keep the job
active with a reconciliation note. The job is not restarted. Container/VM loss
cannot restore live GDB processes.

Each debugger session serializes tokenized MI commands under one ownership lock.
Full transcripts are bounded and stored as artifacts. A timed-out MI command
invalidates the session because its side effects may be unknown.

PoC verification uses a fresh working directory and read-only input copy per
trial. `job.status.result` records `passed`, `attempts`, `successes`, raw per-trial
exit status/signal, input and target SHA256, producer/verifier IDs, environment,
and the observed stage (`crash` or `behavior`). Repeated native crashes do not
prove arbitrary read/write or code execution. Crash minimization requires the
original module-content/location signature by default. Explicit
`allow_signal_only` permits weaker signal/exception equivalence and labels it as
such; independent root-cause confirmation is still needed.

Input reduction uses pinned Picire 26.11 rather than a local delta-debugging
implementation. Existing replay owns every candidate execution and its evidence.
The original baseline consumes one attempt; Picire's reassertion of an already
accepted candidate reuses that trial. Empty input is checked explicitly. A
timeout, cancellation, exhausted attempt budget or unresolved candidate retains
only a previously verified result, with `complete: false` and `stop_reason`.
Unexpected evaluation errors fail the job. `reducer: "picire"` identifies the
algorithm. Doctor advertises minimization only when Picire and its dependencies
are present. These are Linux worker dependencies and are not shipped in the Windows package. The native Windows route uses the Rust `fixture3-ddmin` library and WinDbg MCP
for contained trials; local checks passed and installed-VM acceptance remains
pending. Remove the
previously ignored `trials` and `expected` fields from minimization calls.

On 2026-11-14, actual isolated Linux/GDB acceptance passed with Picire 26.11. Both the static fixture and a dynamically linked fixture faulting inside its admitted main image reduced 23 bytes to `BAD` in 22 attempts with `complete: true` and `location_and_frames`. The dynamic integration also passed typed debugger checks, replay and three verification trials; both isolated LaunchSpec input cases passed. The acceptance image was `sha256:0887afcc06e9ae0df50bd2f7c4b560a9faa7accd032c5c72514b2323ef53a2e6`; the dynamic report SHA256 was `96c4426c5690179db45f4173eff98b722366712593cd00a822ee34be19886e38`. A separate `abort()` fixture correctly refused strong reduction when its PC belonged to unadmitted libc; a module path does not establish a content hash. The fixtures do not qualify arbitrary library identity or AFL fuzzing.

`ghidra.query` accepts the same `query` object as the Rust adapter:
`{"operation":"functions","pattern":"main","offset":0,"limit":100}`,
`{"operation":"decompile","address":1234}`, or
`{"operation":"xrefs","address":1234,"direction":"both","limit":100}`.
Addresses are module-relative. The worker verifies the uploaded SHA256 and binds
cache/project identity to the target, Ghidra version/properties, launcher, bridge
script and analysis settings. Queries reuse imported programs under a project
lock. A lost lock owner requires process reconciliation before a stale lock is
removed. Configure controller request timeout above the query timeout for longer
imports; the transport supports up to 300 seconds.

<a id="验收检查"></a>

## Acceptance checks

Portable trusted fixture process tests:

```powershell
python workers/tests/run.py unit -p test_bridge.py -v
cargo test --test runtime_jobs
```

Actual Linux GDB, replay, independent verification and minimization:

```powershell
& $docker exec wukong-worker python3 /opt/wukong/tests/run.py integration --state /state/acceptance --fuzz
```

Actual Ghidra headless import, analysis, functions/decompile/xrefs queries, cache
identity and hash-mismatch rejection:

```powershell
& $docker exec wukong-worker python3 /opt/wukong/tests/run.py ghidra --state /state/ghidra-acceptance
```

The `ghidra` test suite compiles its own small C fixture, imports it through the
read-only Ghidra worker adapter, resolves `main` and a helper function,
decompiles `main` (the C code must contain the `memcpy` data flow), checks the
caller xref, proves that identical repeat queries hit the identity-bound cache
and that a mismatched target hash is rejected. The target is never executed.

The acceptance test compiles its own small C fixture, breaks at `main`, reads stack
and memory through GDB/MI, reconnects the controller, replays its `abort()` input,
verifies it three times, minimizes it to `BAD`, and (with `--fuzz`) runs real AFL++
QEMU and checks execution counters in its statistics file. It writes the complete
records to `<state>/acceptance-results.json`. It does not attack external systems.

Actual check on 2026-11-06: portable Windows tests passed 6/6. The isolated Linux
acceptance test passed GDB, reconnect, replay, verification 3/3, minimization to
three bytes, and real AFL++ QEMU (17,059 executions in eight seconds, one saved
SIGABRT crash). The Ghidra acceptance test passed headless import, auto-analysis,
functions/decompile/xrefs queries, cache identity and hash-mismatch rejection on
Ghidra 12.2 with Temurin JDK 25. Raw records are in `docs/history/validation/2026-11-06/runtime-linux.json`
and `docs/history/validation/2026-11-06/ghidra-linux.json`. These are historical records, not acceptance of the current commit.

That historical Docker Desktop host's core pattern invokes an external handler. AFL++
rejects that setting by default. The acceptance fixture explicitly sets
`AFL_I_DONT_CARE_ABOUT_MISSING_CRASHES=1` to test QEMU execution/statistics without
changing host-global settings; its output records this override. It does not prove
complete crash collection under that setting. For crash-complete campaigns use a
dedicated Linux VM with an appropriate core pattern, or make an explicit per-job
decision about the same AFL override.

References: [GDB/MI](https://sourceware.org/gdb/current/onlinedocs/gdb.html/GDB_002fMI.html),
[AFL++ QEMU](https://github.com/AFLplusplus/AFLplusplus/blob/stable/qemu_mode/README.md),
[Ghidra 12.2 release and checksum](https://github.com/NationalSecurityAgency/ghidra/releases/tag/Ghidra_12.2_build).


<a id="ghidra-项目归属"></a>

## Ghidra project ownership

The controller uses the standard-library file lock; Python Ghidra workers use
filelock. `worker.lock` is a persistent lock inode and must not be deleted while
an analyzer may own it. The lock covers analyzer execution, cancellation/reaping
and cache publication. An `analysis.active.json` claim survives an owner crash
or uncertain execution failure. Inspect and reconcile the analyzer before
clearing that claim; absence of a lock owner alone does not prove its child exited.

<a id="模块边界"></a>

## Module boundaries

`bridge.py` retains its CLI arguments and disables bytecode writes before importing `wukong_worker.cli`. Deploy the complete `workers/runtime` directory with the sibling generated protocol schema; no additional runtime-package installation is required. Internal imports use the `wukong_worker` package explicitly. `rpc/` owns framing, contract validation, errors and callable registration; `host/` owns identity, capability observation, environment policy and service assembly. Identity hashes the bridge and all nested package source paths/bytes, plus the shared schema and analysis package.

Within the package, `resources/` owns jobs, sessions and durable effects; `execution/` owns admission plans, supervisor execution, process-tree IO, replay and minimization. `debugger/` owns GDB/MI and interactive processes; `artifacts/` owns confined paths, hashes, transfers and atomic metadata. `platform/` retains Linux isolation and the Windows identity/path helpers used by trusted local fixtures. `workbench/` contains host-invoked helpers; control/prepare support direct script execution with an explicit entry bootstrap. The analysis and collection scripts injected into isolated execution remain self-contained.

`analysis/adapters.py` keeps separate `SeccompService`, `LibcService` and `EmuService` admission paths in one file. `debugger/crash.py` retains live GDB observations. They adapt inputs to the independent [analysis package](../analysis/README.md), which owns BPF decoding/probes, bounded ELF candidate comparison, Unicorn simulation and crash-data interpretation, with no worker state or transport.
`network/service.py` validates the prepared service binding and performs declarative
TCP verification. The job supervisor retains lifecycle/cancellation ownership;
network replay never executes a model-provided program inside the privileged
supervisor. See [Pwn tool contracts](../../docs/reference/pwn-tools.md).

`execution/launch_spec.py` owns shared LaunchSpec parsing, confined member lookup and
streamed artifact hashes. Local launch and network replay retain their own
execution policy and explicit resource budgets. `network/service.py` coordinates each
trial; `network/plan.py` validates steps and tracks captures,
`network/transport.py` owns bounded TCP IO and fresh-response boundaries,
and `network/evidence.py` persists exact transmitted/received chunks and their
hash-bound transcript. Cancellation remains owned by the job supervisor.

## Resource ownership

| Resource | Owner and lifetime |
| --- | --- |
| Daemon and endpoint | `rpc/transport.py` holds the daemon guard independently of stdio relays; reconnecting does not restart an owned job. |
| Job reservation and receipt | `resources/jobs.py` serializes deduplication, UID/directory reservation and supervisor launch; `execution/planning.py` prepares normalized inputs before that lock. Failed/unknown launches retain their durable identity. |
| Job process tree and output | The supervisor owns cancellation and final publication; `execution/processes.py` owns process-group/Job termination and bounded output draining. A dead leader is not proof that descendants exited. |
| GDB and interactive sessions | `resources/sessions.py` retains live owners; session classes serialize their operations and retain observations. Recovery marks former sessions lost and reports unreadable metadata without repairing it. |
| Network trial | `network/service.py` owns the socket context and final transcript publication; transport checks cancellation/deadlines and preserves stale/fresh response boundaries. |
| Reduction | `execution/minimize.py` owns the attempt/deadline budget; the injected evaluator owns each execution and crash equivalence. An interrupted candidate is not a minimality claim. |
| Experiment files | `platform/isolation.py` stages validated public inputs under a distinct UID; artifacts are collected through the same confined identity. Stored evidence survives execution. |

Management `doctor` binds the actual registry against disposable state before observing capabilities. It never constructs a runtime against the running daemon's state, which would incorrectly recover live sessions. Closing a transport, cancelling execution and deleting retained evidence are separate operations with separate owners.

Session cleanup persists its intent before worker I/O. Linux process owners record boot/namespace, process creation identity, process groups and owned scratch paths; restart recovery scans these journals even if session publication was interrupted. A missing worker session is not cleanup confirmation: uncertain ownership stays `recovery_required`, and closing/recovery sessions or terminal jobs with `cleanup_pending` block completion and environment maintenance. Controller reconciliation retries closing sessions and resources of failed/stopped owners without replaying debugger commands. Windows retains its native ResourceManager. PID identity/namespace mismatches require environment recovery rather than signalling an unrelated process. Imported journals never grant destination process ownership.
