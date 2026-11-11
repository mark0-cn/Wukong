<a id="桌面安装与更新"></a>

# Desktop installation and updates

English · [简体中文](desktop-updates.zh-CN.md)

Windows uses per-user NSIS EXE; Linux uses AppImage. Tauri merges platform configuration automatically. Online installers do not embed Docker images. Local build: `python -m scripts.release.build_installer --platform windows` (use `linux` on Linux). Signed builds add `--signed` and require `TAURI_SIGNING_PRIVATE_KEY`.

New desktop installations place configuration/data under Windows `%LOCALAPPDATA%/WukongData` or Linux `$XDG_DATA_HOME/Wukong` (default `~/.local/share/Wukong`). Explicit/existing portable configuration retains priority. Uninstall does not delete analysis data/Docker resources by default. Linux support follows the desktop acceptance matrix; AppImage does not promise every distribution.

<a id="信任与状态"></a>

## Trust and state

`updates/manifest.rs` verifies manifest/file signatures, versions, size, SHA256, fixed image digest, runtime protocol and data schema. Release `latest.json` serves Tauri and environment management. Verify its detached signature first, then pass the same bytes to Tauri `restore_update`, avoiding versions changing between queries. Only the current project's version-specific HTTPS attachments and public `hlmark0/wukong` digests are allowed. Signing keys differ from model keys and never enter images.

`desktop/updates.rs` manages checks, downloads, independent candidate self-tests, installation and progress. Preparation never replaces the active worker. Before installation, cache/verify the current installer and atomically save the recovery helper/`updates/pending.json`. The new process changes this to `previous.json` only when its version matches and environment doctor passes. Unconfirmed updates block new tasks. Shared/exclusive `maintenance.lock` coordinates GUI, CLI and updates. Model settings/challenge environments are not replaced; only schema 3 is accepted, with no incompatible migration.

`updates/recovery.rs` does not require WebView: Windows uses the verified previous NSIS installer; Linux atomically replaces AppImage. Check the actual client version afterward and retain records on failure. The user confirms recovery through the independent helper; timeouts never justify overwriting a possibly running installer. Configuration recovery changes only tool-environment fields, preserving model settings. Recovery does not undo challenge data changes.

Cache cleanup protects current, recovery and pending versions. Image cleanup also excludes any container references/shared protected digests; no Docker prune. Pulls continuously parse layer events, sampling every half-second and refreshing UI each second. Show ready/discovered layers, individual states, elapsed time and manifest image version; label local reuse separately. Non-TTY output lacks a complete byte total, so layer counts are not percentages. Retain at most 256 layers/2 KiB per line alongside full logs. Mark pull complete only after successful process exit; self-test completes separately. Client installers show byte progress. Interrupted temporary client downloads restart; Docker layers can be reused.

<a id="docker-预检与诊断"></a>

## Docker preflight and diagnostics

Before candidate preparation, check daemon, Linux container mode and credentials helper needed for the target image. Registry-specific `credHelpers` precedes `credsStore`; honor `DOCKER_CONFIG`. Existing local images need no pull credential check. Windows children retain PATH; if standard Docker CLI is unavailable, try existing Desktop installations and add the selected CLI's sibling directory. Do not replace custom CLIs or modify global PATH/login configuration/credential storage.

Record check/pull/self-test/activation stages and preserve the failed stage. Long-operation output goes to full logs; errors include path and at most 4 KiB/30 tail lines. Do not echo login configuration on read errors. Candidate cleanup failures do not replace the original pull/test failure; log cleanup separately.

UI separates client version from readiness; first setup does not expand update/maintenance actions. Strip extended Windows drive prefixes only for display; convert UNC to conventional network paths. Unedited values retain their native identity; device paths remain unchanged.

<a id="可配置存储位置"></a>

## Configurable storage

Tasks/evidence use `state_dir`; UI switches reopen the workspace through configuration transactions without moving old databases. Large update files use `updates/cache-location.json` to select a cache. Locks, recovery helper and pending/previous records stay in the fixed control directory. Cache changes require exclusive cache/maintenance locks, an empty destination with no ancestor/descendant overlap, signature-verified manifest/installer copying, then an atomic location pointer. Preparation/recovery hold shared cache locks, preventing mid-download changes. Old files remain for later user cleanup. Docker owns image/volume locations.

<a id="发布与验证"></a>

## Publication and validation

`build-installer.yml` signs NSIS/AppImage using already-validated desktop Release binaries on both platforms. Linux desktop/build packaging uses Ubuntu 22.04; the acceptance CLI is separate. Native install/uninstall runs only on isolated runners; Linux starts real WebView through Xvfb. `desktop_manifest` combines image acceptance/installers; `wukong verify-update --directory dist` validates the whole set. Stable pointers advance only after official Release publication; manual rehearsals do not change the update channel.

Local checks cover real signature tamper rejection, maintenance exclusion, manifest origins, frontend controls/layout. Report Linux Docker acceptance separately from native package smoke checks; neither is complete Windows Docker Desktop cross-version acceptance.

Docker info/inspect/version/ps queries have 30-second limits; other bounded control commands allow 600 seconds. Pull/build use separate long-operation paths. stdout/stderr go to temporary files with a continuously checked combined 16 MiB cap before reading into memory. Timeout/overflow reaps this CLI process.

Update state includes operation_id, increasing revision and backend busy. Frontend polling is serial and rejects stale operation requests/revisions. Refresh connection errors are separate from operation failures; reconnection clears only the former.

Pull elapsed time and time since actual layer-state change are distinct; repeated lines do not reset the latter. At 120 seconds without a new layer state, suggest diagnostics without declaring failure or canceling.

Health separates environment record, Docker connectivity, container running and tool self-test. Light checks query Docker/container identity only; full checks run probes. Unchecked values are unknown; record individual check times. Configuration existence is not current availability.

Updates, first setup and repair share a desktop-level operation controller/progress component. Tool environment defaults to health cards, with full observations in folded technical details. Switching entry points does not start another operation.

Preparation can continue in the background, with sidebar progress and reattachment when reopened. Cancellation carries operation_id and rejects stale tasks/install-or-switch stages; seal cancellation under the same state lock before preparation ends. Network requests abort; bounded Docker commands/pulls cancel cooperatively and reap CLI processes. Announce cancellation only after owned candidate cleanup; cleanup failure is explicit. Docker daemon may retain/continue shared downloads; retries reconcile image/cache state, not promise byte-level pause. Starting an environment with a missing image redirects to visible preparation instead of hidden pulling.

While preparing, compress instructions/fold noncurrent steps and layer details. Panel content scrolls independently; bottom actions remain visible and can themselves scroll in very small windows.

Diagnostics classify service/credentials/network/rate/disk/unknown. Text matching gives suggestions, not a proven root cause. Backend records/validates log directories inside this cache's probes; frontend passes only operation IDs, never arbitrary paths. Sanitized diagnostic JSON uses a strict field whitelist: platform, operation identity, stage, category, counts and health booleans. Omit all free text, URLs, paths and raw output rather than regex-based secret masking. Original local logs remain accessible; do not read/package model or Docker login configuration.

Environment operations invalidate old connectivity/self-test observations; only successful activation after a real self-test records new success/time. Other-process workbench locks fail quickly rather than hanging health queries. Background entries retain running preparation/results/errors, not permanent notices after automatic update checks. Health is bound to workspace configuration epoch; changing configuration/workspace invalidates old success as evidence for the new environment.

Local data cleanup lives in the independent `cleanup` service, shared through its CLI. `managed-locations.json` records configuration/cache switches without credentials; newly created state, local-worker and Ghidra roots have an ownership marker. Inventories use known application children, preserving unknown siblings and unverified external directories. File identity and inventory fingerprints are checked before deletion; ordinary files precede metadata, settings and location records. Preview is read-only and does not initialize Store or worker clients. The CLI is an operator interface, never an Agent tool.

`desktop/cleanup.rs` exposes only preferences, preview, confirm and cancel in an isolated webview profile; it never assembles the workspace service. Existing React Modal, locale catalog and typed bridge render the shared inventory, bounded to 200 displayed paths/resources/retained items while keeping the complete review server-side. Confirmation saves the plan outside application data and exits; the native hook runs the headless executor after WebView locks are released. `packaging/windows/uninstall-hooks.nsh` uses Tauri's supported hooks without copying its NSIS template. Upgrade/passive paths do not clean; shared cleanup disables the stock recursive app-data sweep. Matching `cleanup finish` clears deferred admission after native program removal. Linux removes only managed AppImage/launcher/icon integration; portable copies remain.

Docker cleanup reuses the bounded Workbench Docker client, validates local sockets, workspace labels and stable identities, and rechecks retained-container references before deletion. No force image removal or prune. Minimal environment records/imported copies survive for retained Docker scopes/shared resources. Pending updates, active maintenance/workbench locks and local worker shutdown refusal block deletion. Partial file failures preserve remaining metadata; native uninstall aborts with a report. `tests/acceptance/check_cleanup.py` refuses destructive native/Docker acceptance outside disposable GitHub-hosted runners. `cleanup-acceptance.yml` builds an unsigned test NSIS package; ordinary Linux CI tests running fixture containers/shared-resource retention. These do not establish Windows Docker Desktop cross-version acceptance.

For graphical startup diagnostics in automation, `--json` retains stderr and suppresses startup error dialogs; it does not bypass the native review or its selection checks. The native acceptance test verifies the actual embedded window through Windows UI Automation, then checks the saved scope and exit status.
