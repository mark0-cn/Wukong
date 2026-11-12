<a id="执行-worker"></a>

# Execution worker

English · [简体中文](README.zh-CN.md)

The Rust controller never runs an analysed target by default. Choose a persistent
Docker/VM/SSH worker. `trusted_local` explicitly runs trusted fixtures on the host
and is only suitable for development tests. The Python bridge manages processes;
the container or VM supplies isolation.

For Linux workers and trusted development fixtures, install bridge dependencies in the Python environment used by the worker
(or `environments.<id>.runtime.python` for trusted local tests):

```sh
python -m pip install -r workers/runtime/requirements.txt
```

Managed Windows VMs require the complete offline package, including Python and all dependencies; do not fetch packages or run online pip installation in the VM. The [package contract](../package/README.md) and [Windows components](../windows/README.md) describe the current implementation and remaining installer/production acceptance work.

Bridge startup disables Python bytecode writes before importing worker modules, including in new daemon/job-supervisor processes. This keeps signed deployment files unchanged; writable state remains in the configured state directory. Launch other Python entry points from an immutable deployment with `-B` as well.

The Docker image installs these automatically. GDB/MI results include a parsed
`payload`; asynchronous events are structured records with type, message, payload
and the original `record`. Original MI transcripts remain available as evidence.

## Docker

From the repository root (PowerShell):

```powershell
python -m scripts.toolchain.fetch_worker_deps
$docker = 'C:\Program Files\Docker\Docker\resources\bin\docker.exe'
$env:PATH = (Split-Path $docker) + ';' + $env:PATH
& $docker build -f workers/Dockerfile -t wukong-worker:0.1 .
& $docker volume create wukong-worker-state
& $docker run -d --name wukong-worker --network none --cap-drop ALL `
  --cap-add CHOWN --cap-add SETUID --cap-add SETGID --cap-add KILL --cap-add SYS_PTRACE --cap-add DAC_OVERRIDE `
  --security-opt no-new-privileges --security-opt seccomp=unconfined `
  --pids-limit 256 --memory 4g --cpus 2 --read-only --tmpfs /tmp:rw,size=1g,exec `
  --mount type=volume,source=wukong-worker-state,target=/state wukong-worker:0.1
```

Keep the container running to preserve sessions. The controller uses `docker exec`
stdio without publishing a network port. Files are uploaded into the named volume;
the host filesystem and model credentials are not mounted. Add a stronger VM
boundary for kernel experiments. Do not attach the Docker socket to the worker.

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
isolated Linux guest. Copy this directory to `/opt/wukong/runtime`; configure a
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

Relay-managed daemon startup allows 30 seconds for initialization, including the Windows boot-identity query. Readiness probes have a separate one-second response timeout; a child that exits before readiness reports its exit status and at most the final 2048 bytes of the local `daemon.log`.

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
prove arbitrary read/write or code execution. Crash minimization preserves the
signal/exception signature; independent root-cause confirmation is still needed
to distinguish two bugs producing the same signal.

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

`bridge.py` only selects stdio relay, daemon or detached job supervision. Deploy
the complete `workers/runtime` directory; the Docker image already copies it as
a unit. `worker_transport` owns authenticated framing and daemon startup;
`worker_runtime` assembles and routes services. `worker_operations` owns durable
reservation and replay, with session liveness supplied by `worker_sessions`.
`worker_jobs` manages job records and cancellation; `worker_job_supervisor` owns
detached execution. Debugger, interactive processes, files and synchronous
commands have separate components. `worker_processes` contains bounded process
IO and process-tree ownership; `worker_common` contains protocol values, confined
paths, streamed file hashes and atomic metadata IO. Backend adapters import these components directly.

`worker_seccomp` owns offline classic-BPF decoding and concrete probes;
`worker_libc` compares bounded supplied ELF candidates without executing them.
`worker_network` validates the prepared service binding and performs declarative
TCP verification. The job supervisor retains lifecycle/cancellation ownership;
network replay never executes a model-provided program inside the privileged
supervisor. See [Pwn tool contracts](../../docs/reference/pwn-tools.md).

`worker_launch_spec` owns shared LaunchSpec parsing, confined member lookup and
streamed artifact hashes. Local launch and network replay retain their own
execution policy and explicit resource budgets. `worker_network` coordinates each
trial; `worker_network_plan` validates steps and tracks captures,
`worker_network_transport` owns bounded TCP IO and fresh-response boundaries,
and `worker_network_evidence` persists exact transmitted/received chunks and their
hash-bound transcript. Cancellation remains owned by the job supervisor.
