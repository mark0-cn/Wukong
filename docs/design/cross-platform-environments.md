# Cross-platform analysis environments

English · [简体中文](cross-platform-environments.zh-CN.md)

## Environment and task bindings

The controller owns the environment registry, model credentials, task database and evidence store. Each registered environment declares its platform and transport; connecting is lazy, so an unavailable environment does not prevent another environment from running. `environment_list` reports configuration and connection state, not readiness. `environment_doctor` checks the worker's actual operating system and tools. A platform mismatch rejects the connection.

An assessment snapshots its environment configuration. The root uses `default_environment`; native `task` accepts `environment_id`, otherwise inheriting its parent's binding. The binding is immutable. Deduplication includes environment identity alongside role and scope. Environment configuration changes reject further execution in that assessment rather than silently selecting another worker. Children may probe only their assigned environment; root may probe registered environments in the assessment.

For example, a Linux Ghidra specialist and a Windows debugger specialist can share an original PE artifact without sharing worker filesystem paths. Role permissions remain root/analyst/verifier; selecting an environment never grants permissions or activates an unavailable backend.

Before each child model request, an asynchronous worker preflight intersects the role's registry definitions with observed capabilities and platform requirements. Rig holds only that request while probing; root orchestration remains lazy, and cancellation drops the pending probe. A failed or timed-out probe leaves controller tools available and includes an explicit diagnostic and unavailable-tool requirements in the request. Tool execution checks the backend again, including the requested debugger engine and location-preserving minimization mode. An absent capability is denied before worker dispatch, even for a manually constructed call. Linux-only tools are excluded from Windows selections. Local configured Ghidra uses the controller; remote Ghidra requires a successful launcher/bridge/Java availability probe. Capability observations indicate availability, not successful analysis or completed acceptance.

The configuration has no top-level `[runtime]` section. Register transports under `environments.<id>.runtime` and select `default_environment`; an empty registry enables controller-only static analysis. Old single-runtime configuration is rejected without conversion. Workbench Docker CLI configuration lives under `[workbench].docker_executable`; generated workbench configuration registers a Linux environment named `workbench` while preserving other registered environments. Desktop initialization and health checks use the selected environment, so a configured Windows VM does not require Docker. `wukong doctor` reports each registered environment independently.

```toml
default_environment = "linux"

[environments.linux]
name = "Linux analysis"
platform = "linux"

[environments.linux.runtime]
backend = "docker"
docker_container = "wukong-worker"
```

Controller job/session references are qualified by worker identity and retain the worker's local resource ID separately. File registration keys include run, worker and path. Background job observations select the owner's bound environment and use the same evidence/notification pipeline as manual queries.

Protocol 2 handshakes establish the installation ID, operating-system boot identity and worker code/backend content hash. The controller pins this identity on connection and persists it transactionally on first use by each assessment/environment. Reconnection or resume rejects a changed identity before execution. An optional `installation_id` in environment configuration also pins the registered installation. A daemon restart has a separate diagnostic ID; it does not imply an OS reboot or establish session survival.

Production Windows environments require an exclusive 120-second run/controller lease with persistent increasing epochs. A separate transport renews used leases every 15 seconds, independently of model/debugger requests and the controller's own database heartbeat. Workers validate identity and fencing before durable receipt lookup; renewal does not change a business request's fingerprint. Expiry alone never proves process death: in-flight requests block takeover, and another run cannot acquire while owned sessions/jobs remain. Close and reconcile resources before release. Explicit trusted fixtures bypass production leasing, not identity checks.

## Shared and large artifacts

`artifact_stage` accepts a same-run evidence ID and stages a verified immutable copy in the caller's environment. Cross-environment transfer passes through the controller. The original evidence identity is retained. `stage_target` also streams its hash-verified original rather than placing the complete file in one RPC.

The worker exposes `file.upload.begin`, `file.upload.status`, `file.upload.write`, `file.upload.commit`, `file.stat` and `file.read`. Uploads are capped at 16 GiB, chunks at 1 MiB, and the existing RPC frame limit remains 48 MiB. Transfer identity binds destination, size, SHA256 and executable policy. Status is queried independently of cached begin receipts. Repeated or partially persisted chunks must match existing bytes; gaps and conflicting contents are rejected. Commit verifies size and full hash before atomic create-if-absent publication. Existing destination contents cannot be overwritten.

Large downloads verify each bounded chunk and the complete hash before streaming evidence ingestion. `file_get` returns metadata and an evidence ID, not inline file bytes. Model-facing output remains bounded. Actual free disk space is checked at upload preparation; an IO failure can still occur if other processes consume space. Reconnection does not replay unknown debugger mutations.

## Structured debugger adapters

`debug_start` selects `auto`, `gdb_mi` or `dbgeng`; `mode = "dump"` requires a separate staged dump and forbids launch arguments. The staged target remains the run's original image. `debug_inspect` exposes bounded registers, stack, memory, modules, threads, disassembly, exception and execution state. `debug_control` provides typed breakpoints and run/continue/interrupt/step/next/finish. Execution waits for a stop; deadlines request interruption and invalidate the session if a stopped state cannot be established. Dump sessions reject execution control. Raw `debug_command` remains GDB/MI-only.

The native Windows helper loads the configured, hash-checked DbgEng library and accepts only structured operations. The adapter hashes target, dump, helper and engine and retains bounded JSONL transcripts. Local symbol paths exclude servers and environment-provided symbol paths. Real x64/x86 trusted fixture acceptance is separate from production isolation. Until the managed broker exists, Windows execution requires explicit trusted-fixture mode and is not advertised as production-ready.

## Windows offline delivery contract

The native offline bootstrap/service implementation is described in the [Windows component guide](../../workers/windows/README.md); real VM installation acceptance and the managed target-isolation broker remain incomplete. Registration does not establish production availability. The delivery contract requires a complete offline package assembled on the internet-connected controller from pinned, verified official payloads. The VM installer must include every required runtime, dependency and SSH/debugger installation payload and must never fetch missing components. Installation, repair, upgrade and rollback must work without VM internet access; the SSH management connection remains available. Symbols are prepared by the controller and staged into a local-only symbol cache.

The target backend is DbgEng with structured inspection/control, owned sessions and independent replay. The VM supplies the external isolation boundary; target permissions and metadata protections remain separate controls. Kernel debugging, VM creation, automatic snapshot management and GUI interaction are outside the first release. Real Windows VM acceptance is required before claiming Windows execution or offline installation support.

## State and validation boundary

New resource records require state schema 4. Old workspaces are rejected without migration; create a fresh workspace and retain old data separately. Unit/fixture checks exercise task binding, lazy connection, real bridge file transfer, resumable upload and evidence identity. They do not establish real GDB/Ghidra/DbgEng acceptance or Windows VM installer acceptance. See the [implementation contract](implementation-contract.md) and [worker guide](../../workers/runtime/README.md).
