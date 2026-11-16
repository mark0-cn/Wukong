# Windows experiment lab lifecycle

[简体中文](windows-lab.zh-CN.md)

## Scope

Phase 1 of the Windows capability plan gives the controller a bounded lifecycle for one registered VMware experiment VM: prepare a baseline and a per-experiment snapshot, fence every generation, restore to the baseline, and reconcile an interrupted or timed-out host operation. It also locks and verifies controller-side dependency packages and validates install/cleanup plans. The operator entry point is `wukong lab`. It does not yet compile harnesses in the VM, install services or drivers, or expose debugger, network and fuzz tools.

## Registered identity and configuration

Only `[windows_lab.vms.<id>]` entries can be addressed. `vmx` and `vmrun` are operator-supplied absolute paths and the adapter builds every argument; model input never provides a VMX, a `vmrun` flag or a host command.

```toml
[windows_lab.vms.lab]
vmx = "D:/vm/win10/Windows 10 x64.vmx"
vmrun = "D:/VMware/VMware Workstation/vmrun.exe"
baseline_snapshot = "wukong-lab-baseline"
snapshot_prefix = "wukong-lab"
operation_timeout_seconds = 300
start_after_restore = true
```

An ID is 1–64 ASCII letters, digits, underscores or hyphens. Snapshot names allow letters, digits, `.`, `_` and `-`; the prefix must differ from the baseline and stay short. Operations are bounded by 30–3600 seconds.

## State and generation

State lives in `<state_dir>/windows-lab/<vm_id>.json` and records the schema, VM identity, generation, phase, baseline, active experiment, pending operation, last error and update time. A cross-process lock serializes operations and every replacement is published atomically.

Phases are `idle`, `preparing`, `ready`, `restoring`, `recovery_required` and `failed`.

- `prepare` is accepted only while `idle` or `failed`. It advances the generation, persists a pending intent, creates the baseline only when absent, creates one `<prefix>-<lab_id>` snapshot, and reaches `ready` with an active lease.
- `restore` requires the active lab ID and generation. It advances the generation before asking VMware to revert, clears the active experiment, reverts to the baseline, optionally starts the VM, and reaches `idle`.
- A stale generation is rejected before any host call, so an old lease cannot regain execution rights even when the VM is restored.
- A definitive prepare failure becomes `failed` and may be retried. Restore failures and every timeout become `recovery_required`.
- Finding a pending `preparing` or `restoring` state on process start means a controller restart interrupted a host effect. The state becomes `recovery_required`; nothing is replayed.
- `reconcile` resolves that exact operation ID as `completed`, `failed` or `unknown` after the operator inspects the VM.

## Experiment description

`ExperimentSpec` binds `target_sha256`, `launch` (`exe`, `dll_harness`, `service`, `gui`, `driver`), `input_channel` (`argv`, `stdin`, `file`, `network`, `named_pipe`, `com`, `gui`, `ioctl`), `account` (`user`, `elevated_user`, `system`, `driver`), `os_build`, `toolchain_sha256` and a bounded recovery note. Hashes must be 64 lowercase hexadecimal characters. The specification hash is bound into the lease and active state. A valid description is an admission precondition, not execution evidence.

## Preparation pipeline

Dependencies and installation are part of the experiment description:

```json
{
  "dependencies": [
    {"name": "harness.c", "sha256": "<64 lowercase hex>",
     "source": {"kind": "path", "path": "D:/lab/harness.c"}}
  ],
  "install": {
    "targets": ["C:\\lab\\harness.exe"],
    "cleanup": [{"kind": "file", "target": "C:\\lab\\harness.exe"}]
  }
}
```

- A dependency source is an absolute existing `path` or an `http`/`https` `url`. Each artifact is bounded to 64 MiB, a package to 512 MiB, and a download to 60 seconds.
- Every byte is verified against the declared SHA-256. The package ID is the hash of the canonical manifest, and the package is published under `<state_dir>/windows-lab/packages/<id>/` with `manifest.json` and `SHA256SUMS`.
- Reuse re-verifies every existing byte. A mismatch fails instead of overwriting; operators remove a broken package explicitly.
- `install.targets` are absolute Windows paths without dot segments. A service binary must be one of them, `kernel_driver` requires `launch=driver`, and every target, service and account needs a covering cleanup action (`file`, `directory`, `service` or `account`).
- `wukong lab package` materializes and verifies a package without touching the VM. `lab prepare` materializes it before any state change and binds `package_sha256` and `install_sha256` into the pending intent, active experiment and lease.
- A verified package and a valid install plan are admission inputs, not qualification. Harness compilation, dry-run admission and guest installation remain later phase work.

## Host operations

The adapter uses only `vmrun -T ws list`, `listSnapshots <vmx>`, `snapshot <vmx> <name>`, `revertToSnapshot <vmx> <name>` and `start <vmx> nogui`. Each call has a timeout and captured stdout/stderr, never a shell. Baseline snapshots are created only when absent; existing snapshots are never deleted.

## Limits

- The CLI must run from a normal-integrity directory. A build tree can carry a Low mandatory label, and VMware refuses to start under it. The desktop publish script already copies executables outside the workspace; manual CLI runs should do the same.
- Guest installation still needs the documented offline media and an operator/admin path. This lifecycle never guesses passwords and never edits VM disks or accounts.
- `start_after_restore` must match how the host restores a running snapshot: on this host a running-VM snapshot restores powered off, so the lab starts it when configured.
- Experiment snapshots are not deleted automatically; operators remove them explicitly after preserving evidence.
- Phase 1 validation used the real VM: baseline and experiment snapshots were created, restores to the baseline ran in generations 2 and 4, and the worker `doctor` reported ready again after the VM start. Controller-restart, timeout and stale-generation behavior are covered by unit tests with a fake host.

## CLI

```powershell
wukong --config wukong.toml lab status --vm lab
wukong --config wukong.toml lab prepare --vm lab --owner analyst --purpose parser --spec spec.json
wukong --config wukong.toml lab restore --vm lab --lab <lab_id> --generation <n>
wukong --config wukong.toml lab reconcile --vm lab --operation <operation_id> --outcome completed
```
