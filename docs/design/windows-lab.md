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

Only the current state format and tool contracts are supported. There is no migration workflow or compatibility layer for earlier branch-local formats. OS and toolchain descriptions use optional `annotations.os_build` and `annotations.toolchain` notes.

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

The administrative installation channel consumes the experiment spec's hash-pinned `install.ps1` and package bytes; worker setup media is only consumed by the offline provisioning command described below. The controller verifies the package again, copies verified bytes into a temporary host snapshot, transfers them into `C:\Windows\Temp\wukong-lab-<install_id>`, and invokes a fixed receipt wrapper. A single 1–600 second deadline covers all administrative commands. Cancellation kills the host command but may leave guest effects; these remain uncertain.

The guest wrapper verifies media hashes, waits for a separate PowerShell script process, records its exit code, and checks declared target files, service binary/type/running state and account existence. It publishes an identity-bound JSON receipt which is retrieved and retained in the installation record. A successful host `vmrun` exit is not installation success. Missing/mismatched receipts stay unknown; completed success requires guest exit zero and passing postconditions. Script identity, guest receipt and cleanup requirement are distinct from the install-plan hash. Guest observations are evidence from the disposable guest, not an independent exploit verifier.

## Offline worker provisioning

`wukong lab provision` installs the Windows worker once into a registered disposable lab VM while that VM is powered off. It is an operator-only CLI action: no agent tool, experiment spec or model output can supply its paths or commands. The command re-verifies finalized worker media, writes a fixed provisioning wrapper, plan, per-operation boot launcher and a fixed Windows Task Scheduler boot-trigger task into the offline system volume through WSL `guestfish`/`hivexsh`, attaches a `WUKONG_OFFLINE` ISO to a dedicated SATA CD device, and boots the VM so the scheduled task runs the wrapper as `NT AUTHORITY\SYSTEM` without a login: the wrapper installs the worker, unregisters its own fixed boot task and boot launcher and powers the guest off. The controller then restores the original VMX bytes, reads the guest receipt offline, creates the configured baseline snapshot and optionally registers and verifies the controller-managed SSH environment. The guest never receives a controller key or an administrator password. The feature is additive to schema-2 lab state: phase `provisioning`, pending-operation kind `provision` and the `last_provision` record; existing idle/failed state files load because the added record defaults to absent.

The boot mechanism is a Windows Task Scheduler boot-trigger task, not a local Group Policy startup script and not a Windows service. Real-VM evidence rejects both earlier mechanisms: a PowerShell process registered as a service never calls `StartServiceCtrlDispatcher`/`SetServiceStatus`, so SCM kills it after 30 seconds (events 7009/7000) before the wrapper can run; and the local Group Policy client filtered the machine policy out as empty at every recorded boot (Group Policy Operational events 4000/5312/5313/8000, message "local Group Policy not applied (empty)"), so a `scripts.ini` entry under `Machine\Scripts\Startup` never executed and the PowerShell Operational log recorded zero events during the acceptance boot. Provisioning now writes the fixed task `\WukongLabProvision` (boot trigger with a one-minute delay) with the XML task file under `C:\Windows\System32\Tasks` and the `Tree`/`Tasks`/`Boot` values under `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Schedule\TaskCache` in `C:\Windows\System32\config\SOFTWARE`. The fixed GUID, `D:P(A;;FA;;;BA)(A;;FA;;;SY)` security descriptor and `powershell.exe -File C:\ProgramData\WukongLab\provision\launch.ps1` action are canonical: the action path never changes, while the per-operation `launch.ps1` names the operation wrapper so leftovers stay attributable. Task Scheduler's `Hash`/`Triggers`/`Actions` values are its internal binary serialization and cannot be regenerated from XML with documented APIs, so the registration is a captured canonical fixture replayed byte for byte (captured from a real registration and cross-checked against a Windows 10 reference volume). Only the fixed task file and TaskCache keys are written: no SYSTEM service key and no local-GPO files. The retired local-GPO injection code (including the `bump_gpt_version` half-word mask bug) was deleted; cleanup still recognises and removes retired artifacts byte-exactly.

Current validation covers planning and fake host/disk/ISO/guest-network/environment adapters, including scheduled-task registration planning, the wrapper finally ordering, the explicit guest cleanup command and the stale-leftover refusal path. The canonical task file and TaskCache values were also proven against a real Windows Task Scheduler store: they were injected offline, the Task Scheduler service was restarted, the task was enumerated and executed as `NT AUTHORITY\SYSTEM`, and deleting the registration from inside the running wrapper action did not terminate it. No real-VM boot on a lab guest has been executed by CI and no real WSL/libguestfs injection, `mkisofs` build or VM boot has been executed for this revision; real acceptance of the replacement remains operator-run on a dedicated VM.

### Prerequisites

- Windows controller; other platforms fail closed with an explicit error. WSL commands run as the distribution's `root` user.
- `[windows_lab.vms.<alias>.offline]` with `wsl_executable` and `mkisofs` as absolute Windows paths, `wsl_distribution` naming one registered WSL distribution, `guestfish`/`hivexsh` as absolute POSIX paths inside that distribution, and `timeout_seconds` (60–1800) bounding each tool command. The distribution must provide libguestfs `guestfish`, `hivexsh`, `wslpath`, `sha256sum`, `tee` and coreutils, and reach the VM's VMDK through `wslpath`. A missing section is an actionable error; the controller never assumes a host tool layout.
- A stock Windows 10/11 guest whose Task Scheduler service is enabled and can read the Task Scheduler cache. Provisioning needs read/write access to `C:\Windows\System32\Tasks`, `C:\Windows\System32\config\SOFTWARE` and `C:\ProgramData\WukongLab` on the powered-off disk. It does not use a Windows service, the guest administrator password or the administrative channel.
- `python` on `PATH` and the command run from the wukong checkout when `--template` is used, because provisioning invokes `python -m scripts.worker.media finalize` itself. `--media` provisioning does not use Python.
- Keep the controller `state_dir` (and the finalized media output directory) reasonably short: Windows `MAX_PATH` also applies to staged offline media, whose payload contains 131-character relative paths. Finalize stages in a short temporary workspace and fails with an explicit error naming the offending path when the configured directory is too deep; move the directory to a shorter absolute path instead of relying on long-path opt-in.
- A registered lab VM that is powered off and has no snapshot chain. Only the configured `baseline_snapshot` may exist, and only with `--replace-baseline`; any other snapshot rejects the command and is never deleted. When no snapshot exists, `--replace-baseline` is a documented no-op. The boot disk must be a full local VMDK inside the VMX directory on `nvme0:0`, `scsi0:0` or `sata0:0`; delta, linked and foreign disks are rejected. The VMX must enable its SATA controller and have a free CD slot.
- VMware Tools when `--register` is requested: the controller discovers the guest IPv4 with `vmrun getGuestIPAddress` before writing the environment.
- A finalize spec whose `management_peer` is the controller IPv4 source address as the guest sees it. The installed worker pins SSH to exactly that address (authorized-key `from=`, `AllowUsers user@address`, owned firewall rule), so any other address leaves the environment doctor unreachable.
- For `--register`, an active configuration file: pass the global `--config` option or run where `wukong.toml` is discoverable.

```toml
[windows_lab.vms.lab]
baseline_snapshot = "wukong-lab-baseline"

[windows_lab.vms.lab.offline]
wsl_executable = "C:/Windows/System32/wsl.exe"
wsl_distribution = "Ubuntu"
guestfish = "/usr/bin/guestfish"
hivexsh = "/usr/bin/hivexsh"
mkisofs = "D:/VMware/VMware Workstation/mkisofs.exe"
timeout_seconds = 600
```

### Media inputs

Exactly one source mode is accepted:

- `--media DIR` — an already finalized media directory. Provisioning re-verifies it before touching the guest disk: schema-1 `media.json` receipt, setup/enrollment/manifest SHA-256, signed enrollment and payload manifest, `disposable_vm` execution boundary and the media SSH identity. The directory contains `WukongWorkerSetup.exe`, `setup.json`, the sealed `payload/`, `enrollment/{enrollment.json,enrollment.sig}`, optional `recovery/` and `media.json`; links, reparse points, unknown entries, more than 20,000 entries or more than 4 GiB are rejected. The directory is read-only input and is not modified or deleted.
- `--template DIR --keys DIR --spec FILE` — a released media template plus the absolute controller signing key directory and the finalize spec (schema 1: 32-character lowercase hex `installation_id`, `environment_id`, `generation`, canonical IPv4 `management_peer`, `ssh_port`, only the `disposable_vm` boundary and optional `ssh_public_key`). Provisioning runs finalize into a per-provision directory under the controller state directory, generates the controller SSH identity when the spec omits `ssh_public_key`, then re-verifies the produced media and cross-checks it against the spec. The key directory must be outside the template and output; only public media bytes are published, and the finalized media is removed after success or reconciliation.

The release asset `wukong-windows-worker-media-<version>.zip` is an unsigned template with no enrollment; operators finalize it locally with their controller keys. See [offline Windows worker media](../development/releases.md#offline-windows-worker-media) for the asset and [native Windows worker components](../../workers/windows/README.md) for the media layout, spec fields and standalone `finalize`/`pack`/`iso` commands. A desktop installation ships `worker-setup/WukongWorkerSetup.exe` ([bundled worker setup](../development/desktop-updates.md#bundled-worker-setup)) but not worker media; provisioning always needs a finalized media directory or the released template.

### Provisioning sequence and baseline

Before any mutation the command validates the source media/template and spec, the VM power state, the snapshot rules and the VMX CD planning. A pristine VM additionally preflights the powered-off system volume (Windows volume, SYSTEM and SOFTWARE hives, current control set, no fixed scheduled-task file or TaskCache registration or boot launcher, no provision directory, no retired local-GPO startup entry/backup or CLI-created `gpt.ini`, and no legacy one-shot service key) before anything is persisted. A detected leftover refuses the command with the exact explicit cleanup command (`wukong lab clean-guest --vm <alias> --operation <id>`) instead of silently retrying over it. With `--replace-baseline` the powered-off VM still points at the baseline snapshot's delta, so the guest disk can only be resolved and preflighted after that snapshot is deleted: the command backs up the VMX and persists the intent first, then deletes exactly the configured baseline and validates the committed disk. Every failure after the intent is persisted becomes recoverable rather than replayed.

The mutation order is fixed: back up the original VMX bytes and persist the intent before any VM or guest mutation; when `--replace-baseline` was passed, delete exactly the configured baseline snapshot and only then resolve and preflight the committed guest disk; build the ISO with `-V WUKONG_OFFLINE`; attach the ISO on the free SATA CD device; write the wrapper, plan, per-operation boot launcher and fixed task XML to the fixed guest paths; register the fixed boot task by creating its `Tree`/`Tasks`/`Boot` TaskCache keys in the guest SOFTWARE hive; start the VM and wait for guest power-off within `--timeout` (60–7200 seconds, default 1800; individual tool commands use `offline.timeout_seconds` instead). The Task Scheduler service starts the task as SYSTEM without a login, the launcher runs the operation wrapper, and the wrapper's `finally` block writes its receipt first, then unregisters the fixed task with `schtasks /Delete` (which removes the task XML file and all three TaskCache keys), deletes the fixed launcher, schedules removal of the wrapper and plan, and always powers the guest off. The controller restores the VMX, verifies offline that the task file, TaskCache registration, launcher, wrapper and plan are gone, that the retired local-GPO artifacts are absent and that the receipt identity matches, creates the baseline snapshot and leaves the VM powered off. No SYSTEM-hive service key, local-GPO file or service value is written by this path. The durable provisioning record stores every guest file the operation wrote together with whether it was created by the operation or preserved from pre-existing content, so cleanup can delete created artifacts and restore preserved ones byte for byte after the pending intent has been resolved.

After provisioning the VM is powered off at the new baseline. The normal order is then `lab prepare` (which snapshots the baseline for one experiment generation) and `lab run` (a packaged harness through the registered managed-SSH worker), with `lab restore` returning to the baseline. `lab prepare` refuses while `last_provision` is running or unknown. `--register` writes the `[environments.<id>]` entry but does not edit `[windows_lab.vms.<alias>]`; set its `environment` to the registered environment id before running packaged harnesses.

`--replace-baseline` is required exactly when the configured baseline already exists; when no snapshot exists it is a documented no-op (the pristine flow creates the baseline after the guest receipt verifies). With the baseline present, the command backs up the VMX and persists the recovery intent, deletes exactly that snapshot, and only then resolves and preflights the committed guest disk; it recreates the baseline only after the guest receipt verifies, and no other snapshot is ever deleted. A failure after deletion leaves no baseline; the operation stays `recovery_required`, names the VMX backup, and must be reconciled before re-running provisioning (add `--replace-baseline` only if the baseline exists again).

### Registration and environment doctor

`--register` requires `--template` mode. Finalize generates the controller-managed SSH keypair (`--generate-ssh-key` under the provisioning directory in the controller state directory) only when the finalize spec omits `ssh_public_key`; media-mode provisioning cannot supply that identity, and a spec that already carries `ssh_public_key` cannot be registered. The private key stays under the controller state directory and is referenced by path from configuration; it is never read into media, receipts or the guest.

After the guest boots, the command waits for the guest IPv4 from `vmrun`, writes `[environments.<environment_id>]` as a managed-SSH Windows environment (installation id, `wkgm...` user, SSH port, controller identity-file path, public known-hosts pin derived from the verified receipt host key), writes a timestamped backup of the previous configuration first, refuses to replace a differing environment without `--force`, sets `default_environment` when it is unset and retracts the replaced host pin. `--verify-environment` requires the environment to be configured (by `--register` or manually) and runs `EnvironmentRegistry::doctor` after boot, retrying until the deadline; it is valid with either media mode and can be combined with `--register`. On success the VM is reverted to the new baseline. A registration or doctor failure is reported separately: the command attempts to revert the VM, the baseline is retained and the provisioning record shows `environment_verified: false`.

### Dry run, failures and reconciliation

`--dry-run` performs the same read-only preflight and prints the exact planned actions (media re-verification, ISO path, VMX backup and CD device, fixed guest files, scheduled-task file and TaskCache registration, boot launcher, boot/power-off, VMX restore, baseline creation, receipt path) without finalizing template media, building the ISO, editing the VMX, writing guest files, creating snapshots or publishing provisioning state or receipts. The VM must still be powered off and the offline tooling must be configured; the pristine plan inspects the disk read-only. With `--replace-baseline` and an existing baseline the read-only plan stops before the guest disk: the powered-off VM still points at the snapshot delta, so the plan reports the VMX backup, intent persistence and baseline deletion, and that the disk is validated only after the deletion; it does not inspect the guest system volume. `--iso-tool` overrides the configured `mkisofs` executable for one command (absolute path; configuration is unchanged); `--force` is valid only with `--register`.

A failure before the intent is persisted does not touch the VM, the guest or snapshots and creates no provisioning record; template finalization may leave unpublished working files under the per-provision directory. Any failure after persistence moves the VM to `recovery_required`, keeps the pending `provision` operation and writes a `last_provision` record with status unknown; the error carries the operation id and receipt (`lab_provision_unknown`). The VMX may still reference the generated ISO and the guest may hold partial provisioning state, so the operator must inspect the VM and power it off before `wukong lab reconcile --vm <alias> --operation <operation_id> --outcome completed|failed|unknown`. `completed` and `failed` restore the original VMX bytes and remove the generated ISO and finalized media before resolving the operation; `unknown` keeps it unresolved. Reconciliation never re-runs the guest installation or invents a passing receipt. A controller restart converts an unfinished pending provisioning operation into `recovery_required` on the next mutating lab command; nothing is replayed automatically.

Reconciliation does not edit the guest disk. Before retrying provisioning, remove the failed attempt's guest leftovers with `wukong lab clean-guest --vm <alias> --operation <operation_id>`. The command requires the VM to be powered off and the offline tooling; `--dry-run` prints the planned removals without writing. It removes only that operation's `C:\ProgramData\WukongLab\provision\<id>` tree, the fixed `\WukongLabProvision` scheduled-task registration (task XML file and the TaskCache `Tree`/`Tasks`/`Boot` keys) and its fixed `launch.ps1` boot launcher when they belong to that operation, plus artifacts of the retired local-GPO mechanism: the `wukong-provision-<id>.cmd` startup launcher, its `[Startup]` entry and the per-operation `scripts.ini`/`gpt.ini` backups. A backed-up policy file is restored byte for byte (no trailing newline is appended); a `gpt.ini` that the retired CLI created without a backup is deleted only for the last recorded operation and only while its bytes still equal the exact created document; the injected `[Startup]` entry is removed when no backup exists. It also removes the legacy `WukongLabProvision-<id>` service key written by earlier attempts. It refuses unknown or foreign operation ids, never deletes other operation directories, unrelated policy/registry content or unrelated tasks, is idempotent and is bounded by the configured offline tool timeout. A stale leftover detected by a new provisioning preflight is refused with this exact command rather than silently retried.

### Security invariants

- Controller private keys stay on the controller: `--keys` is consumed only by the local finalize step; only the signed enrollment, sealed payload and setup executable enter the media/ISO; template and output are checked to exclude the key directory and private-key names; `--register` stores only the controller-side identity path.
- No guest administrator password and no administrative channel is used: the offline path injects the fixed scheduled-task file and its TaskCache registration and installs from the CD; the configured admin channel and password environment variable remain for administrative installation scripts only. Provisioning does not write a service: `hivexsh` edits the guest SOFTWARE hive only for the fixed TaskCache keys, and read-only SYSTEM-hive probing plus the explicit guest cleanup command handle the legacy one-shot service key. No local Group Policy file is touched; the wrapper unregisters the fixed task before shutdown and `clean-guest` removes it when a failed attempt leaves it behind.
- Inputs are hash-pinned end to end: the media receipt, signed enrollment and payload manifest are verified before and after finalize, again on the host before boot, and again by the guest wrapper; the guest receipt read back binds provision id, installation id, environment id, generation, manifest and worker host key. `production_isolation_verified` is always false because a guest receipt cannot certify hypervisor isolation.
- The VMX is always restored: the original bytes are saved before the CD edit, restored on success before the baseline snapshot and restored by reconciliation on failure; reconcile refuses while the VM is powered on.
- Paths and commands are fixed: only `C:\ProgramData\WukongLab\provision\<32 lowercase hex>` paths, the fixed `C:\Windows\System32\Tasks\WukongLabProvision` task file, the fixed `{A861DA08-B3C7-44C1-8BAC-579B48F54859}` TaskCache identity with the `D:P(A;;FA;;;BA)(A;;FA;;;SY)` security descriptor, the fixed `launch.ps1` boot launcher and fixed Task Scheduler action, the per-operation `provision.ps1`/`provision-plan.json`, and (for cleanup only) the retired `wukong-provision-<id>.cmd`/`scripts.ini.wukong-lab-<id>.bak`/`gpt.ini.wukong-lab-<id>.bak` paths and legacy `WukongLabProvision-<id>` service name are addressed; host tools receive fixed argv without a shell; media, template, key, ISO and VM paths are validated absolute; tool and provisioning waits are bounded; model or experiment input never reaches this module.
- Re-runs are guarded: a leftover fixed scheduled-task registration/task file/launcher, provision directory, retired startup policy entry/backup or legacy one-shot service key blocks provisioning and names the guest cleanup command, an existing guest receipt prevents reinstallation, only the configured baseline may be replaced and no other snapshot is deleted.

### CLI example

```powershell
# One-time: finalize the released template, provision, register the managed-SSH environment and run its doctor.
wukong --config wukong.toml lab provision --vm lab `
  --template D:/release/wukong-windows-worker-media-<version> `
  --keys D:/wukong-controller-keys --spec enrollment-spec.json `
  --replace-baseline --register --verify-environment

# Validate and print the planned actions without finalizing media or touching the VM.
wukong --config wukong.toml lab provision --vm lab --media D:/media/windows-worker --dry-run

# Provision from finalized media without environment registration; override mkisofs once.
wukong --config wukong.toml lab provision --vm lab --media D:/media/windows-worker `
  --iso-tool "D:/VMware/VMware Workstation/mkisofs.exe" --timeout 2400

# Resolve a failed provisioning intent after inspecting the powered-off VM.
wukong --config wukong.toml lab reconcile --vm lab --operation <operation_id> --outcome failed

# Remove only that operation's leftovers from the powered-off guest disk, then retry.
wukong --config wukong.toml lab clean-guest --vm lab --operation <operation_id> --dry-run
wukong --config wukong.toml lab clean-guest --vm lab --operation <operation_id>
wukong --config wukong.toml lab provision --vm lab --media D:/media/windows-worker
```

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

Offline worker provisioning is covered by unit and planning tests with fake host, disk, ISO, guest-network and environment adapters, including VMX CD/disk planning, the canonical scheduled-task file/TaskCache fixtures (pinned sizes, SHA-256 values and hivexsh scripts), the wrapper receipt contract and finally ordering, the explicit guest cleanup command (idempotent, dry-run capable, foreign-id refusal, byte-exact policy restore and created-file deletion), stale-leftover refusal, media/template validation, provisioning state recovery and registration/configuration writing. No test writes a SYSTEM service key or a local-GPO startup entry through the provisioning path. The canonical task layout was proven offline against a real Windows Task Scheduler store (offline injection, service reload, SYSTEM execution and self-unregistration) and cross-checked against a Windows 10 reference volume; a real-VM boot acceptance run of this replacement remains operator-run, and no real WSL/libguestfs injection, `mkisofs` build, VMware boot, guest scheduled-task execution or managed-SSH `doctor` run has been executed by CI for this revision.

## CLI

```powershell
wukong --config wukong.toml lab status --vm lab
wukong --config wukong.toml lab prepare --vm lab --owner analyst --purpose parser --spec spec.json
wukong --config wukong.toml lab restore --vm lab --lab <lab_id> --generation <n>
wukong --config wukong.toml lab reconcile --vm lab --operation <operation_id> --outcome completed
wukong --config wukong.toml lab clean-guest --vm lab --operation <operation_id> [--dry-run]
```

Use a normal-integrity directory for VMware command execution; a Low-integrity build tree can prevent VMware from starting. `start_after_restore` should reflect the host's actual powered-off/running snapshot behavior.
