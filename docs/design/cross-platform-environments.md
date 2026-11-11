# Cross-platform analysis environments

English · [简体中文](cross-platform-environments.zh-CN.md)

## Environment and task bindings

The controller owns the environment registry, model credentials, task database and evidence store. Each registered environment declares its platform and transport; connecting is lazy, so an unavailable environment does not prevent another environment from running. `environment_list` reports configuration and connection state, not readiness. `environment_doctor` checks the worker's actual operating system and tools. A platform mismatch rejects the connection.

An assessment snapshots its environment configuration. The root uses `default_environment`; native `task` accepts `environment_id`, otherwise inheriting its parent's binding. The binding is immutable. Deduplication includes environment identity alongside role and scope. Environment configuration changes reject further execution in that assessment rather than silently selecting another worker. Children may probe only their assigned environment; root may probe registered environments in the assessment.

For example, a Linux Ghidra specialist and a Windows debugger specialist can share an original PE artifact without sharing worker filesystem paths. Role permissions remain root/analyst/verifier; selecting an environment never grants permissions or activates an unavailable backend.

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

## Shared and large artifacts

`artifact_stage` accepts a same-run evidence ID and stages a verified immutable copy in the caller's environment. Cross-environment transfer passes through the controller. The original evidence identity is retained. `stage_target` also streams its hash-verified original rather than placing the complete file in one RPC.

The worker exposes `file.upload.begin`, `file.upload.status`, `file.upload.write`, `file.upload.commit`, `file.stat` and `file.read`. Uploads are capped at 16 GiB, chunks at 1 MiB, and the existing RPC frame limit remains 48 MiB. Transfer identity binds destination, size, SHA256 and executable policy. Status is queried independently of cached begin receipts. Repeated or partially persisted chunks must match existing bytes; gaps and conflicting contents are rejected. Commit verifies size and full hash before atomic create-if-absent publication. Existing destination contents cannot be overwritten.

Large downloads verify each bounded chunk and the complete hash before streaming evidence ingestion. `file_get` returns metadata and an evidence ID, not inline file bytes. Model-facing output remains bounded. Actual free disk space is checked at upload preparation; an IO failure can still occur if other processes consume space. Reconnection does not replay unknown debugger mutations.

## Windows offline delivery contract

The Windows backend and installer are subsequent implementation work; registration does not establish their availability. The delivery contract requires a complete offline package assembled on the internet-connected controller from pinned, verified official payloads. The VM installer must include every required runtime, dependency and SSH/debugger installation payload and must never fetch missing components. Installation, repair, upgrade and rollback must work without VM internet access; the SSH management connection remains available. Symbols are prepared by the controller and staged into a local-only symbol cache.

The target backend is DbgEng with structured inspection/control, owned sessions and independent replay. The VM supplies the external isolation boundary; target permissions and metadata protections remain separate controls. Kernel debugging, VM creation, automatic snapshot management and GUI interaction are outside the first release. Real Windows VM acceptance is required before claiming Windows execution or offline installation support.

## State and validation boundary

New resource records require state schema 4. Old workspaces are rejected without migration; create a fresh workspace and retain old data separately. Unit/fixture checks exercise task binding, lazy connection, real bridge file transfer, resumable upload and evidence identity. They do not establish real GDB/Ghidra/DbgEng acceptance or Windows VM installer acceptance. See the [implementation contract](implementation-contract.md) and [worker guide](../../workers/runtime/README.md).
