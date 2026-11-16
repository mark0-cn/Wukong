# Windows experiment lab lifecycle

English · [简体中文](windows-lab.zh-CN.md)

## Scope and ownership

The controller manages a registered disposable VMware VM, verified offline packages, prebuilt user-mode harnesses, and administrative installation scripts. Host paths and credentials come from operator configuration. Models cannot supply VMware commands, controller file paths or guest administrator credentials. Harness compilation and generation of service/driver implementations are outside this lifecycle.

The implementation separates typed records (`types`), atomic persistence (`store`), host lifecycle, packaged execution (`execution`), package/media validation and worker/admin adapters. Tool handlers receive concrete services. Assessment execution uses the ordinary job ledger; asynchronous host operations use the existing durable effect ledger.

```toml
[windows_lab.vms.lab]
vmx = "D:/vm/win10/Windows 10 x64.vmx"
vmrun = "D:/VMware/VMware Workstation/vmrun.exe"
environment = "windows"
baseline_snapshot = "wukong-lab-baseline"
snapshot_prefix = "wukong-lab"
operation_timeout_seconds = 300
start_after_restore = true

[windows_lab.vms.lab.admin]
user = "Administrator"
password_env = "WUKONG_LAB_ADMIN_PASSWORD"
```

VM aliases contain 1–64 ASCII letters, digits, underscores or hyphens. Snapshot names additionally allow dots; the experiment prefix must differ from the baseline. Host commands have a configured 30–3600 second bound. The adapter builds fixed arguments, runs without a shell and bounds returned output. The password is read from its named environment variable and excluded from state and receipts; `vmrun` necessarily receives it on its command line on the trusted controller.

## State, identity and resource fencing

Schema-2 state lives in `<state_dir>/windows-lab/<vm_id>.json`. It binds the canonical VMX identity and configuration fingerprint, baseline, generation, active experiment, pending host operation and execution receipts. Atomic replacement preserves the previous state on failed publication. Physical VM identities own the operation lock and alias registration; duplicate aliases and shared environment mappings are rejected. Changing the VMX, worker environment or other bound configuration cannot reuse an existing lease.

Assessment worker bindings hold a shared lock beside the canonical VMX (`.wukong-environment.lock`). Prepare/install/restore require exclusive admission. Before maintenance, the caller may release only its own idle assessment lease, after all job evidence is synchronized. A fresh native worker lease prevents takeover while orphan jobs or debugger/interactive resources remain. A powered-off VM needs no worker connection. Other assessments' connections and leases are never closed by a lab tool. After a baseline restore, the old assessment's worker identity remains pinned; further execution requires a fresh assessment if the boot identity changed.

- `prepare` admits only `idle` or `failed`, validates/materializes inputs, advances the generation, persists intent, creates a missing baseline and an experiment snapshot, then publishes a `ready` lease.
- `restore` validates the caller, active lab ID and generation before recovery writes. It fences the old generation before reverting. Unknown run/install records become `lost` only after restoration succeeds. Failure or timeout preserves uncertainty. Successful restore attempts to prune its experiment snapshot and reports pruning failures separately.
- `prune` rejects pending recovery and preserves baseline/active snapshots. It deletes only unused snapshots with the configured prefix.
- `status` reads a snapshot without reconciling or rewriting it; bounded host probes are observations. It remains usable while another operation holds the mutation lock.
- Interrupted host effects are never replayed. Mutating recovery classifies interrupted intents as `recovery_required`; an operator must inspect the guest and reconcile the exact operation ID. Ordinary worker starts are blocked by unresolved maintenance effects, while observation and cleanup remain available.

Schema-1 files are deliberately rejected: finish/reconcile the old experiment with its original version, preserve its evidence, and explicitly archive the old lab state before creating schema-2 state. Do not copy an old lease to another VM or silently edit its identity. Move old `os_build` and `toolchain_sha256` declarations into optional `annotations.os_build` and `annotations.toolchain` notes.

## Experiment and package contracts

The CLI `ExperimentSpec` contains a verified `target_sha256`, typed launch/input/account requirements, optional descriptive `annotations`, dependencies, install plan and recovery note. Notes are not enforced environmental requirements or observed facts. Harness admission supports only `launch=exe`, `account=user`, and `input_channel=stdin` or empty `argv`; other requirements are rejected before dispatch. The selected executable must match the bound target hash.

Agent `lab_prepare` has a typed `spec`. It derives ownership from assessment/agent/generation, uses the assessment target unless `target_evidence_id` is supplied, and accepts dependencies as `{name,evidence_id}`. The service resolves evidence within that assessment, computes hashes and materializes controller copies. Models neither invent target hashes nor supply an `owner`, URL or controller path. All new tool argument objects reject unknown fields.

CLI operators may provide dependencies with an absolute existing path or HTTP(S) URL and a SHA-256. Limits are 256 members, 64 MiB per artifact, 512 MiB per package and 60 seconds per download. The package ID is the canonical manifest hash. Every reopen for execution or media verifies that identity, member names, size budgets and member bytes, rejecting links/reparse paths. Changing both a member and its manifest cannot rebind the original package ID.

Install plans declare absolute Windows target paths and optional service/account postconditions. A service binary must be a declared target; kernel services require `launch=driver`. Cleanup actions are optional ownership notes. Baseline restoration is the cleanup boundary; scripts are not automatically reversed.

## Tool handles and execution

`lab_list` lists configured aliases; `lab_status` requires `vm` and returns one VM's state. Child agents can address only their assigned VM/environment, using the configuration captured at assessment creation. Leases cannot be reused by another agent or generation.

`lab_prepare`, `lab_install` and `lab_restore` return `operation_id` promptly. Poll `lab_operation_status` for the result and original evidence. Preparation's result contains an opaque `lab_ref` plus the lease. Run/install/restore take only `lab_ref`, bound to this assessment, agent generation and VM lease; models do not reconstruct lease fields. These mutations require the `windows_lab` task grant. Snapshot pruning remains an operator CLI action. `lab_operation_cancel` requests interruption of an owned operation; cancellation does not prove rollback. A controller restart exposes a missing live operation as uncertain without replay. Active/uncertain host effects prevent the owning agent and assessment from completing. Inspect the guest, use `wukong lab reconcile` for the VM intent when applicable, and use the existing operator `wukong reconcile <operation_id> --result-file ... --evidence-id ...` to resolve its durable effect receipt.

`lab_run` returns `run.job_id` from the normal assessment job ledger. Use `job_status`, `job_cancel` and `job_artifacts`; ordinary supervision renews the same assessment/controller lease. Task completion and evidence import use the existing job rules. The lab receipt records admission identity; job receipts carry final execution evidence. A subsequent restore imports a settled job observation before changing the VM.

The CLI `lab run` waits for the result and maintains its standalone lease heartbeat every 15 seconds. It releases only its own settled scope. Execution is bounded to 1–600 seconds, one executable upload to 32 MiB, stdin to 1 MiB and output previews to 1 MiB. Transport/poll uncertainty retains the existing run identity and blocks replacement execution. No privileged account, GUI, network or driver execution is implied by a descriptive spec.

## Administrative installation evidence

Media requires hash-pinned `install.ps1`. The controller verifies the package again, copies verified bytes into a temporary host snapshot, transfers them into `C:\Windows\Temp\wukong-lab-<install_id>`, and invokes a fixed receipt wrapper. A single 1–600 second deadline covers all administrative commands. Cancellation kills the host command but may leave guest effects; these remain uncertain.

The guest wrapper verifies media hashes, waits for a separate PowerShell script process, records its exit code, and checks declared target files, service binary/type/running state and account existence. It publishes an identity-bound JSON receipt which is retrieved and retained in the installation record. A successful host `vmrun` exit is not installation success. Missing/mismatched receipts stay unknown; completed success requires guest exit zero and passing postconditions. Script identity, guest receipt and cleanup requirement are distinct from the install-plan hash. Guest observations are evidence from the disposable guest, not an independent exploit verifier.

## Driver installation, test signing and Verifier

The administrative channel can install a kernel service from the same hash-pinned `install.ps1`:

```powershell
New-Item -ItemType Directory -Force C:\lab | Out-Null
Copy-Item (Join-Path $PSScriptRoot 'driver.sys') C:\lab\driver.sys
sc.exe create WukongLabDrv type= kernel start= demand binPath= C:\lab\driver.sys
sc.exe start WukongLabDrv
```

A test-signed driver additionally needs guest-local test signing and a trusted test certificate:

```powershell
Import-Certificate C:\lab\test.cer -CertStoreLocation Cert:\LocalMachine\Root
Import-Certificate C:\lab\test.cer -CertStoreLocation Cert:\LocalMachine\TrustedPublisher
bcdedit.exe /set testsigning on
shutdown.exe /r /t 3 /f
```

After the reboot, create and start the service, then enable standard Driver Verifier for it and reboot again:

```powershell
sc.exe config WukongLabDrv start= system
verifier.exe /standard /driver WukongLabDrv.sys
shutdown.exe /r /t 3 /f
verifier.exe /querysettings
```

Test signing is sufficient for test drivers. PatchGuard is not bypassed or disabled by this workflow, and third-party PatchGuard-bypass tooling is not part of the lab; if an experiment needs one, treat it as an isolated operator step and restore the baseline afterwards. The baseline restore removes the test certificate, testsigning flag, service and Verifier settings together.

## Directory bundles and Python scripts

Directory analysis discovers up to 256 files, 512 MiB and 16 levels, hashes members and infers executable/library/data kinds plus import edges. `bundle_manifest` exposes discovery; `bundle_evidence` imports a verified member for static analysis. Entry inference is a recommendation.

`launch_prepare` with `source.kind=bundle` accepts optional `source.entry` and `source.members` (including entry, at most 64), plus `args`, `cwd`, `env`, `input_mode` (`stdin_pipe` or `argv_file`) and a required 1–3600 second timeout. Selected members total at most 32 MiB and use at most five relative path levels. A non-root cwd must exist in the layout; `launch.json` is reserved. `argv_file` requires `@@`; the complete LaunchSpec fits 64 KiB. Invalid selections and launch options are rejected before bundle upload. Entry changes use a separate executable-admission namespace; launch configuration changes receive different content-addressed paths. Call `job_start` with the returned `launch_spec`, omitting argv/timeout_secs/cwd/env overrides.

`py_run` requires exactly one nonempty `source` or `source_file`; the schema rejects missing, empty and combined forms. Linux and native Windows enforce the same 256 KiB UTF-8 byte limit. Native file input is resolved in the admitted owner scope and sealed by content hash before execution. Linux also retains the script under a content-derived path. Neither path bypasses normal job ownership or isolation.

## Static backend selection

```toml
[analysis]
static_environment = "linux-static"
```

Ghidra probes and binds only the chosen static environment; ordinary execution/status/cleanup binds only the primary environment. Both validate the assessment's configuration snapshot and persisted worker identity. Static tools are advertised only after observing that backend's capability, not from a configured name alone. A failed static connection cannot break ordinary worker cleanup, and a failed Windows worker cannot prevent an otherwise available static query.

## Validation limits and historical evidence

The branch history before the audit fixes (`231d180`) recorded real VMware snapshots/restores, a prebuilt EXE run, service installation/removal, a self-signed legacy WDM driver under test signing and standard Driver Verifier, Windows-to-Linux Ghidra analysis, and a two-member `vulnserver.exe`/`essfunc.dll` LaunchSpec run with cancellation. Those are historical observations of the earlier implementation, not acceptance of the new schema, asynchronous handles, lease fences or guest receipt wrapper.

Current regression coverage exercises stale/failed recovery, caller/configuration fences, package tampering, interface schemas and native/Python source admission. Real VMware guest installation and reboot acceptance must be rerun for the final commit. Driver coverage does not include PnP/device I/O, production signing or PatchGuard bypass. Native installer tests belong on isolated runners, not the user's installation.

## CLI

```powershell
wukong --config wukong.toml lab status --vm lab
wukong --config wukong.toml lab prepare --vm lab --owner analyst --purpose parser --spec spec.json
wukong --config wukong.toml lab restore --vm lab --lab <lab_id> --generation <n>
wukong --config wukong.toml lab reconcile --vm lab --operation <operation_id> --outcome completed
```

Use a normal-integrity directory for VMware command execution; a Low-integrity build tree can prevent VMware from starting. `start_after_restore` should reflect the host's actual powered-off/running snapshot behavior.
