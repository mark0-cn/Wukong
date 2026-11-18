<a id="本地发布构建"></a>

# Local-release builds

English · [简体中文](local-builds.zh-CN.md)

`cargo package-local` produces a **local-release** desktop installer: a native NSIS EXE or AppImage that completes first-run onboarding ("prepare tool environment") against a Docker image already built on this host, without the online signed `latest.json` catalog. Use it to test an unpublished checkout end to end.

<a id="为什么需要它"></a>

## Why it exists

Online first-run setup downloads the official `latest.json` for the current client version and requires the signed catalog's `runtime_protocol`/`data_schema` to match this build. An unpublished commit, or a branch whose protocol/schema moved ahead of the published manifest, therefore cannot prepare an environment and reports `unsupported release format, runtime protocol or data migration`. A local-release build replaces that one acquisition with a bundled descriptor; environment preparation, self-check and activation still run through the normal pipeline.

<a id="构建方式"></a>

## Build one

```sh
cargo package-local --check
cargo package-local --output target/installers/local-release-1
```

`--check` validates source/tool entry points only. A real build runs `npm ci`, builds the Windows worker setup on Windows, resolves the local Docker image, writes `target/local-release.json`, bundles it as the Tauri resource `local-release.json`, and builds the client with the `custom-protocol` and `local-release` Cargo features. Re-running the command replaces an existing `--output` directory that only holds previous `package-local` installers; a directory with unrelated files or subdirectories is refused rather than deleted. `--signed` keeps working for updater-signature testing; a signed local-release package is still a development artifact and is never published. `--online` builds the previous, catalog-dependent installer instead.

CI release packaging does not use this mode: `scripts/release/build_installer.py` defaults to the online configuration and no release workflow enables the `local-release` feature. For `tauri dev` or a feature-enabled `cargo run`, Tauri may not copy bundled resources next to the development executable; point `WUKONG_LOCAL_RELEASE_JSON` at an absolute descriptor path to exercise the same runtime path.

<a id="docker-镜像要求"></a>

## Docker image requirement

The packaging host must have a linux/amd64 image for the workbench. The default tag is `wukong:local`; set `WUKONG_LOCAL_IMAGE` to package a different tag (for example the `wukong:0.1` development tag from the source setup guide). When the tag is absent, packaging builds it from `docker-bake.hcl`:

```sh
python -m scripts.toolchain.fetch_worker_deps
docker buildx bake --load --set workbench.tags=wukong:local
```

Onboarding re-checks the image on the Docker host, reads its real size and refuses a missing or non-linux/amd64 image with the build command in the error. The image is never pulled from a registry and the installer never embeds the image, registry credentials or signing keys. Rebuilding the image after packaging is fine; onboarding only warns that the recorded size is stale.

<a id="windows-worker-媒体模板"></a>

## Windows worker media template

If `target/worker-media-template-gui.zip` exists (or `WUKONG_LOCAL_TEMPLATE` names a template archive), packaging records its absolute path as `template_path`. The Windows Worker card then uses that template without downloading the signed release asset; the shared media generator still validates the archive. When no template is packaged the descriptor omits the field and the GUI offers local file picking as usual. The path is only valid on the packaging host, so move or rebuild the template before using the installer elsewhere.

<a id="限制"></a>

## Limitations

- Development only. Do not distribute, publish or sign local-release packages as releases; CI never produces them.
- No online updates: update checks still use the signed catalog and may reject an incompatible published release. Re-run `cargo package-local` for a new local build.
- The descriptor is not signed. Its trust boundary is the local build; it can only steer onboarding to a local image and never replaces an online update identity.
- The accepted Windows worker template path is an absolute host path; it is not portable to another machine.
- The updates/status payload carries `local_build: true` so the UI can mark development builds later.
