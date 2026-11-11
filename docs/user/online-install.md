<a id="安装与应用内更新"></a>

# Installation and in-app updates

English · [简体中文](online-install.zh-CN.md)

[Language settings](languages.md)

Wukong provides a Windows x64 installer EXE and a Linux x64 AppImage. Installation files contain the client; the tool image downloads online on first use. They contain no model credentials or challenge data.

## Windows

1. Download `Wukong_<version>_x64-setup.exe` from an official [GitHub Release](https://github.com/mark0-cn/Wukong/releases); use the actual attachment filename.
2. Choose English or SimpChinese, install for the current user and open Wukong. The client uses WebView2; the installer can prepare a missing runtime online. Application language follows the system initially and can be changed independently.
3. First launch opens environment setup. Install and start Docker Desktop in Linux containers mode.
4. Select **Prepare tool environment**, wait for the image download and self-test, then configure the model in settings.

The EXE currently has no Authenticode signature. In-app updates use separate signature verification.

## Linux

Supports x86_64 desktops; official builds use Ubuntu 22.04. Make the downloaded AppImage executable and run it:

```sh
chmod +x Wukong_*.AppImage
./Wukong_*.AppImage
```

The system must support AppImage execution. If FUSE is missing, install the distribution's compatible FUSE library or use `APPIMAGE_EXTRACT_AND_RUN=1 ./Wukong_*.AppImage`.
After tool setup, **Updates → Maintenance options → Add to application menu and restart** copies the app to the user application directory and creates a desktop entry. This is a user-level installation without sudo.
Install Docker Engine and confirm the current user can access Docker before preparing tools. Wukong does not install Docker, change groups or modify system services automatically.

<a id="首次准备与错误处理"></a>

## First setup and diagnostics

Setup shows **Check Docker → Download tool image → Validate and activate**; client version and tool readiness are separate. The first-run page presents prepare/retry and Docker installation help. Storage belongs in **Workspace settings → Storage locations**; cleanup and recovery belong in **Updates → Maintenance options**, also shown when recovery is pending.

Failures appear once, with a copy control and expandable Docker log tail. Errors from long Docker operations include the complete log path. If `docker-credential-desktop` is missing, fully exit and reopen Wukong; if it still fails, check the Docker Desktop installation and helper directory in PATH. Wukong searches confirmed installation directories for the CLI and sibling helpers, without deleting login configuration or disabling credential storage. This error is unrelated to Windows extended data-path prefixes.

During pulls, view ready/discovered layer counts, elapsed time and per-layer states. Layers differ in size: counts are not download-byte percentages. Environment validation follows the pull. Images use the signed manifest's digest, so Docker Desktop may show a local `<none>` tag; content remains pinned and the client displays the corresponding tool image version.

<a id="环境与数据"></a>

## Environment and data

- Public image `hlmark0/wukong` supports anonymous pulls; you can sign in to your own Docker Hub account if rate-limited.
- Each release manifest binds a fixed image digest; changes to `latest` do not change the running environment.
- Reserve at least 25 GiB for Docker images and temporary layers. Wukong checks the download disk; check Docker Desktop virtual disk capacity separately in its settings.
- Tasks/evidence default to `.wukong` under the user data root below; update cache defaults to `updates`. Windows UI shows conventional drive/UNC paths while preserving native path identity internally.
- New desktop installations use Windows `%LOCALAPPDATA%/WukongData` or Linux `$XDG_DATA_HOME/Wukong`, defaulting to `~/.local/share/Wukong`.
- Explicit `--config` and existing portable configuration retain priority. Installers do not replace configuration, task databases or challenge files.
- `preferences.json` in the fixed user data root stores language preferences independently of workspace configuration.
- History and settings remain available before tool setup; tasks requiring a worker cannot run until it is ready.

<a id="更新"></a>

## Updates

In **Workspace settings → Storage locations**, change:

- **Tasks and evidence directory**: switch to an existing or new workspace. Existing analysis data is not moved automatically; select the original directory to return.
- **Update cache directory**: select an empty directory, including on another disk. Existing signed manifests, installers and recovery files are verified and copied. The location changes only after all steps succeed; originals remain.

Defaults remain under the user data root above. Configuration discovery, maintenance locks and recovery helpers keep fixed locations so relocated caches do not hide recovery records.
Docker Desktop/Engine manages physical image and volume storage; this is separate from client download cache.
Related directories cannot change during analysis, downloads or pending recovery. A CLI `--state` override requires restarting without that override before switching through the UI.

The app checks official releases at startup; failure does not disable a working environment. Open **Updates**:

1. Select **Check for updates** and read release notes.
2. Finish analysis, debugging and file operations before **Download and verify update**. Preparation currently requires an idle desktop; downloads do not replace the active environment.
3. Download the new installer, previous recovery installer and matching image; verify signatures and self-test in a separate container.
4. Save drafts and select **Install and restart**, confirming exit. Active CLI analysis also prevents switching.
5. The new client validates its matching environment and confirms the update. Failure preserves recovery records and blocks new analysis.

Challenge containers and images do not upgrade with tool images. Only compatible data formats are accepted; incompatible database migration is not performed.
Retry network failures: verified installers and Docker layers can be reused; interrupted temporary client downloads restart.

<a id="修复恢复与清理"></a>

## Repair, recovery and cleanup

**Tool environment → Validate and repair environment** uses the current client's official signed manifest to prepare its tools again, without upgrading the client.

An independent recovery program is saved before updating:

- Windows: `%LOCALAPPDATA%/WukongData/updates/Wukong-Recovery.exe`
- Linux: `~/.local/share/Wukong/updates/Wukong-Recovery.AppImage` (honoring `XDG_DATA_HOME`)

Use **Updates → Maintenance options → Restore previous version**. If the app cannot open, close Wukong and run that file. Recovery requires Docker but no new installer download. Failure preserves records for retry; image rollback does not undo challenge data changes.

Clean old client caches and unreferenced old tool images separately. Current, previous recovery and pending versions are protected; no global Docker prune is performed.
Uninstalling does not delete analysis data or Docker resources by default. The Linux user desktop entry is `applications/io.wukong.desktop.desktop` under the user data directory.

<a id="验证边界"></a>

## Validation boundary

CI checks Windows install/uninstall, Linux AppImage and real WebView startup, signature integrity and Linux Docker integration.
Real Windows Docker Desktop first installation and cross-version upgrades still require desktop-machine acceptance. Hosted-runner installer checks and language UI browser tests do not replace that acceptance.

The cleanup CLI can preview application-managed local data without initializing Store, a model or a worker; legacy SQLite ownership checks use immutable read-only connections. Run `wukong cleanup plan --data --output cleanup-review.json`, inspect the paths/counts/bytes and retained items, then explicitly apply it with `wukong cleanup execute --review cleanup-review.json --yes --report cleanup-report.json`. Save these files outside the directories being cleaned. Execution takes the maintenance lock and rebuilds the inventory; changed contents require a fresh review. Unrecognized files and link/reparse paths remain. A configured directory is not permission to delete all its contents.

Windows uninstall opens a scope review: local data/settings, Docker tools and Docker challenges are independent choices. Docker choices are unchecked. The native data checkbox only sets the initial data choice; the detailed review is authoritative. Review shows actual locations, local-file size, owned Docker resources and retention reasons. Cancel leaves the program installed. Default silent uninstall preserves everything except the program; explicit automation flags are `/WUKONG_DELETE_DATA`, `/WUKONG_DELETE_TOOLS` and `/WUKONG_DELETE_CHALLENGES`. `/UPDATE` and passive upgrade paths ignore cleanup requests. Linux `wukong cleanup uninstall` opens the same review, removes managed desktop integration when launched from the installed AppImage, and retains externally downloaded portable AppImages for manual removal.

Data cleanup includes managed tasks, conversation/checkpoint/evidence files, generated artifacts, settings/preferences, framework browser data, local worker/Ghidra caches and logs, verified update/recovery files and tracked custom locations. Unknown siblings, original external attachments, exports, external custom Skill sources, process environment API keys, Docker credentials, shared runtimes and remote environments remain. Unmarked legacy external cache roots are retained when ownership cannot be established. Retained Docker environments keep minimal workspace/location/launch records and imported challenge copies. Shared images, volumes and networks remain while referenced by retained containers. Docker disk usage is not included in the local-file size total.

The CLI adds `--tools` and `--challenges` to `cleanup plan`. Only local `unix://`/`npipe://` Docker sockets are supported for cleanup; do not switch Docker context between review and execution. Finish/cancel active work and confirm/recover pending updates first. Existing local workers are authenticated and asked to shut down; active jobs cause refusal. Cleanup uses maintenance/workbench locks, rechecks identities, deletes selected containers before unused volumes/networks/images, and removes configuration/ownership records last. Failures produce an incomplete report and retain the program during native uninstall; some selected files/resources may already have been removed. Retry after fixing the reported cause. Interrupted native cleanup blocks new work until the uninstaller is retried. No global prune or shared-tool removal is performed.
