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

The packaging host must have a linux/amd64 image for the workbench. `cargo package-local` resolves it in this order:

1. `WUKONG_LOCAL_IMAGE` when set: package exactly that tag, building it from `docker-bake.hcl` when it is absent. This remains the explicit escape hatch.
2. The CI-built image `<namespace>/wukong-workbench:local-<version>` pulled from Docker Hub, verified as linux/amd64 and tagged as `wukong:local`, replacing any older local tag (see the next section).
3. An already present local `wukong:local` tag, used only when that pull is impossible (tag not found, missing authorization or a network failure).
4. A local `docker buildx bake` of the `workbench` target.

`WUKONG_LOCAL_IMAGE_REGISTRY` overrides the workbench repository, whose default is `hlmark0/wukong-workbench`. That repository is intentionally different from the official release image `hlmark0/wukong`, so local packaging can never pull a release tag or a moving `latest` tag; the official repository path is refused. The pulled tag is `local-<version>`, built from the current `Cargo.toml` version, so pull it from a checkout of the same version. The packaging host needs `docker login` only when the Docker Hub repository is private; a public repository pulls anonymously. The CI image is preferred over a stale local `wukong:local` tag, which packaging replaces when the pull succeeds; the local tag is only reused, with a warning that it may be older, when the pull is impossible, and removing it (`docker image rm wukong:local`) makes that fallback a local Bake build instead.

Onboarding re-checks the image on the Docker host, reads its real size and refuses a missing or non-linux/amd64 image with the build command in the error. The installer never embeds the image, registry credentials or signing keys. Rebuilding the image after packaging is fine; onboarding only warns that the recorded size is stale. When the packaging shell itself cannot reach Docker (for example a sandboxed terminal), packaging records `image_size: 0` and defers the presence, platform and size checks to onboarding; set `WUKONG_LOCAL_IMAGE_SIZE` to record a known size explicitly, and build the image from a shell with Docker access when it does not exist yet.

<a id="ci-built-workbench-image"></a>

## CI-built workbench image

Building the workbench image locally compiles a large tool image, so maintainers publish it once per client version with the manual **Actions → Workbench image** workflow instead of every builder doing so:

1. Set `version` to the client MAJOR.MINOR.PATCH version that the selected ref's `Cargo.toml` already declares; the workflow fails when the input differs from the checkout.
2. Optionally set `ref` to the branch, tag or commit to build; the default is the ref selected when starting the run.
3. Run it. The workflow verifies the locked build inputs, builds the same `workbench` Bake target used locally, checks the loaded image is linux/amd64 and that its tool imports work, then pushes `<DOCKERHUB_NAMESPACE>/wukong-workbench:local-<version>`. The run summary records the actor, source ref, commit, digest and size. It never pushes the official `<namespace>/wukong` image and never creates or moves a `latest` tag; it is not an official release.

Builders then only run `cargo package-local`, which pulls the matching `local-<version>` tag and tags it as `wukong:local`, replacing an older local tag. Incompatible changes are handled by bumping the client version: a changed `runtime_protocol`/`data_schema` requires a new version, and therefore a new immutable `local-<version>` tag (the workflow refuses to overwrite an existing version tag). If the pull fails because the tag is missing, the repository is private without a login, or the network is unreachable, packaging prints the maintainer workflow and the local Bake command, then reuses an existing local `wukong:local` tag when present or falls back to the local build. One-time Docker Hub setup is creating the separate `wukong-workbench` repository under the release account; the existing `DOCKERHUB_NAMESPACE`/`DOCKERHUB_USERNAME` variables and `DOCKERHUB_TOKEN` secret (with write access) are reused, and no environment reviewer or actor allowlist gates the run.

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
