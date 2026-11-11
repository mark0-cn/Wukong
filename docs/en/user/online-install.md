# Installation and in-app updates

[简体中文](../../user/online-install.md) · [Usage](usage.md) · [Languages](languages.md)

Wukong provides a Windows x64 installer and a Linux x64 AppImage. The installation includes the client; matching tools are downloaded online as a Docker image. It contains no model credentials or challenge data.

## Windows

1. Download the `Wukong_<version>_x64-setup.exe` attachment from an official [GitHub Release](https://github.com/mark0-cn/Wukong/releases).
2. Choose English or Simplified Chinese and install for the current user. The installer can prepare a missing WebView2 runtime online.
3. Start Docker Desktop using Linux containers.
4. Open Wukong and choose **Prepare tool environment**. Wait for download and validation, then configure your model provider in settings.

The EXE currently has no Authenticode signature. In-app updates use a separate signature verification mechanism. Installer language does not set the application's language preference; the app initially follows the system.

## Linux

The AppImage targets x86_64 desktop systems and is built on Ubuntu 22.04.

```sh
chmod +x Wukong_*.AppImage
./Wukong_*.AppImage
```

If FUSE is unavailable, install the distribution's compatibility library or launch with `APPIMAGE_EXTRACT_AND_RUN=1 ./Wukong_*.AppImage`. Choose **Add to application menu and restart** for a per-user installation. This does not require sudo.

Install Docker Engine and confirm your user can access it before preparing tools. Wukong does not install Docker, change group membership or configure system services automatically.

## Tools, configuration and storage

- The public `hlmark0/wukong` image supports anonymous pulls. You may sign in to Docker Hub if rate-limited.
- Each release binds an immutable image digest. Changes to `latest` do not silently change your environment.
- Reserve at least 25 GiB for Docker images and temporary layers. Wukong checks the download disk; Docker Desktop virtual disk capacity is managed separately.
- Default writable storage is `%LOCALAPPDATA%/WukongData` on Windows and `$XDG_DATA_HOME/Wukong` on Linux, falling back to `~/.local/share/Wukong`.
- Explicit `--config` and existing portable configuration remain supported. Installer replacement does not overwrite task databases or challenge files.
- `preferences.json` in the fixed user data root stores language choices independently of workspace configuration.
- History and configuration remain available before tool setup. Work requiring a worker cannot run until the worker is ready.

In **Workspace settings → Storage locations**, you can switch the task/evidence directory or relocate the update cache. Switching task directories does not move old tasks; choose the old directory to return. Cache relocation requires an empty directory, verifies and copies recovery files, then switches only after success. Original files remain available for review and cleanup.

Configuration discovery, maintenance locks and recovery entry points keep fixed locations. Docker manages image and volume storage independently. Directory changes require idle analysis and maintenance; a `--state` override must be removed by restarting before changing the data directory through the UI.

## Updates

The app checks official releases at startup. Check failure does not disable an already working environment.

1. Open **Updates** and select **Check for updates**.
2. Finish analysis, debugging and file operations before **Download and verify update**.
3. Wukong downloads the new client, the previous recovery installer and matching tool image. It checks signatures and validates a separate candidate environment.
4. Save drafts and choose **Install and restart**. An active CLI assessment also prevents switching.
5. The new client validates the matching environment and confirms the update. Failure preserves recovery records and blocks new analysis until resolved.

Challenge containers and images do not update with the tools. Incompatible database migration is not performed. Failed downloads can be retried; verified files and Docker layers can be reused, while incomplete client downloads restart.

## Repair, recovery and cleanup

**Tool environment → Validate and repair environment** prepares the current version's tools without upgrading the client.

Recovery programs are saved at:

- Windows: `%LOCALAPPDATA%/WukongData/updates/Wukong-Recovery.exe`
- Linux: `~/.local/share/Wukong/updates/Wukong-Recovery.AppImage` (honoring `XDG_DATA_HOME`)

Use **Updates → Maintenance options → Restore previous version**, or close Wukong and run that program if the main app cannot open. Recovery uses verified local installers and requires Docker. Failure keeps recovery records for retry. Image rollback does not undo challenge data changes.

Cleanup is limited to old recorded downloads and tool images without container references. Current, recovery and pending versions remain protected. Uninstalling the client does not delete analysis data or Docker resources by default.

Automated installer checks and Linux Docker acceptance do not replace a real Windows Docker Desktop first-install or cross-version upgrade test. Language UI browser tests also do not replace native installer interaction.
