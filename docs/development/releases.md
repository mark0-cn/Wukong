<a id="在线发布与-github-actions"></a>

# Online releases and GitHub Actions

English · [简体中文](releases.zh-CN.md)

Repository: `mark0-cn/Wukong`. Public image: `hlmark0/wukong`. Distributions: Windows x64 NSIS EXE and Linux x64 AppImage; image platform linux/amd64.

<a id="仓库配置"></a>

## Repository configuration

**Settings → Secrets and variables → Actions**:

| Type | Name | Value |
| --- | --- | --- |
| Variable | DOCKERHUB_USERNAME | hlmark0 |
| Variable | DOCKERHUB_NAMESPACE | hlmark0 |
| Secret | DOCKERHUB_TOKEN | Docker Hub repository Read/Write PAT; Delete is unnecessary |
| Secret | TAURI_SIGNING_PRIVATE_KEY | Tauri updater private key, only for trusted release workflows |
| Variable | RELEASE_APP_ID | Dedicated release GitHub App ID |
| Secret | RELEASE_APP_PRIVATE_KEY | Full PEM private key generated for that App |

Replace expired Docker PATs in Secrets. Never put PATs in source, packages, command arguments or release notes. Clients pull public images anonymously; publishing credentials are CI-only.

One-time App setup:

1. Create an App in [GitHub Apps settings](https://github.com/settings/apps/new), use the repository URL as Homepage, disable Webhook, and restrict installation to your account.
2. Repository permissions: **Contents: Read and write**, **Pull requests: Read and write**. Administration, Actions write and Workflows write are unnecessary.
3. **Install App → Only select repositories → `mark0-cn/Wukong`**.
4. Save App ID in the variable above and the complete generated PEM in the secret. Never commit/send the private key in chat.

Workflows issue short-lived repository installation tokens and revoke them afterward. PR creation requests Contents/PR write; tagging requests only Contents write. App-created PRs trigger native `pull_request` CI; App-pushed tags trigger publishing, with no extra branch CI dispatch. This avoids built-in `GITHUB_TOKEN` PR-approval restrictions and its push events not triggering subsequent workflows. See [GitHub trigger rules](https://docs.github.com/en/actions/how-tos/write-workflows/choose-when-workflows-run/trigger-a-workflow).

Defaults are standard GitHub-hosted `ubuntu-24.04` / `windows-2025`. Image build and acceptance are separate jobs exchanging only digests/small files, not image tars. Disk preparation is restricted to hosted Linux VMs, removes unneeded preinstalled SDKs, prints space and preflights capacity. If necessary, set optional `WORKBENCH_LINUX_RUNNER` to an already-configured larger **GitHub-hosted** runner label; no paid resources are provisioned automatically.

The repository is public and uses standard hosted runners. First uncached image builds are slow. Registry cache uses public Docker Hub `buildcache`; Actions retains only clients/reports, normally 7–14 days. Runner billing and storage allowances follow GitHub's current account policy; larger runners and excess storage must be checked separately.

<a id="本地生成原生安装文件"></a>

## Build native installers locally

Run `cargo package-local` in committed, clean Windows x64/Linux x64 source. It checks source/tools and invokes Tauri for the platform's NSIS EXE/AppImage, outputting `target/local-installers/`. It neither downloads tools nor publishes. First client setup requests the matching official Release's signed manifest, so an unpublished local version cannot fetch its environment through online setup.

```sh
cargo package-local --check
cargo package-local --output target/installers/build-1
cargo package-local --signed
```

Existing output directories are rejected. `--check` checks source/tool entry points; actual builds verify system/installer dependencies. `--signed` requires `TAURI_SIGNING_PRIVATE_KEY`; ordinary local builds need no release secret. Windows needs MSVC, Rust, Node.js/npm and Python; Linux also needs Tauri WebKitGTK, GTK, AppIndicator, patchelf and related build libraries. Official AppImages build on Ubuntu 22.04 and receive real WebView startup checks, not every-distribution guarantees. Legacy ZIP scripts remain development fixtures; official releases/`cargo package-local` use native installers.

Independent launcher formatting: `cargo fmt --manifest-path xtask/Cargo.toml -- --check`.

<a id="日常-ci"></a>

## Everyday CI

`ci.yml` runs on master pushes, PRs targeting master and manual dispatch. PRs select checks by scope; master/manual/version PR/release invocations run full checks. Windows runs Rust fmt/clippy/test, frontend tests/build and online-package validation. Linux runs Python runtime/analysis evaluation tests. External PRs receive no Docker Hub secret. See [GitHub maintenance](github-maintenance.md) for scope/caches.

`rust-toolchain.toml` pins Rust; Cargo/npm use lockfiles. Debian/Python transitive dependencies are not fully locked. Released image digests are authoritative; rebuilding is not guaranteed byte-for-byte reproducible.

<a id="发布版本"></a>

## Publish a version

1. Open **Actions → Prepare Version PR → Run workflow**, choose master and enter a complete tag such as `v0.2.0`. Leave `dry_run` unchecked for normal preparation.
2. Synchronize Wukong versions in `Cargo.toml`/`Cargo.lock`, `tauri.conf.json`, and UI versions in `ui/package.json`/`package-lock.json`. Check syntax, monotonic increase and consistency, run release tests, then create `codex/release-v0.2.0` and a PR targeting master.
3. The App-created PR starts native CI visible on the PR. After Windows/Linux/summary pass, the maintainer approves merging; **merging the version PR authorizes publication**.
4. Master merge CI runs fully and prebuilds Windows Release client, Linux acceptance CLI and Ubuntu 22.04 Linux desktop Release client. After all succeed, **Tag Merged Release** validates/pushes a tag pointing to that checked merge commit, even if master subsequently advances.
5. The tag triggers **Build and Publish**, building, validating and publishing Docker Hub/GitHub Release. No local action/manual tag push is required.

Preparation uses standard hosted runners and the App, not a personal PAT; built-in `GITHUB_TOKEN` is read-only. Ordinary PRs do not auto-tag. Automatic publication accepts only same-repository merged PRs created by the configured App from `codex/release-vX.Y.Z`, whose merge changes only the five version files by a pure version increase.

Before tagging, requery the merge commit's CI/current attempt and require changes/windows/linux/summary success. Failed CI prevents tags. Prepare a new valid version PR after code fixes, or rerun that commit's CI/tag workflow for transient failure. A tag already pointing to the same commit is idempotent; another target is rejected. Retry **Build and Publish** for existing-tag release failures, never delete/recreate the tag. Old `github-actions[bot]`, manually created or pre-feature merged version PRs are not published retroactively.

Preparation creates no tag/image/Release. Version must increase; existing tags are rejected. Retry reuses a version branch only if its tree exactly matches this preparation, without force-push. If master/branch changes cause failure, resolve the existing PR/branch before preparing again.

`dry_run` shows synchronized diff/tests without remote branch/PR. Local `python -m scripts.release.cli bump --tag v0.2.0` synchronizes files for inspection, but use App PR flow for automatic publication. It does not update/resolve dependencies and rejects inconsistent current versions, downgrades or invalid format before editing.

Tags must be `vMAJOR.MINOR.PATCH`, e.g. `v0.1.0`, `v12.3.45`. Components are ASCII digits, allowing standalone zero but no leading zeros. Reject `1.2.3`, `V1.2.3`, `v1.2`, `v01.2.3`, `v1.2.3-rc.1`, `v1.2.3+build`. Tags must match source versions and point to commits included in default branch master.

The workflow listens to all tags and validates metadata before image build/package/publication. Workflow validation does not itself reject invalid Git pushes; server-side enforcement requires an applicable tag Ruleset. Historical-commit tags execute that commit's workflow. Check current repository Rulesets separately rather than assuming workflow filtering is server enforcement.

`release.yml` performs:

1. Validate version/origin. Reuse successful same-repository/same-SHA master push CI only after checking current-attempt changes/windows/linux/summary. Otherwise, including API failure, run full CI.
2. Find trusted same-SHA CI/release clients or build them. Before execution/packaging, verify platform, version, full commit, recipe and binary SHA256/size. Bake stable toolchain/workbench together; verify external assets against `workers/toolchain.lock.json`, even on cache hits. Publish candidate `build-<fullcommit>` with SBOM/provenance.
3. On a fresh runner pull by digest and run network isolation, fixed-target relay, LaunchSpec/archive tests, and host-CLI doctor, challenge import/build/extract/start/connect, GDB attach, update/rollback.
4. Package both platforms' validated desktop binaries directly into NSIS/AppImage without recompiling; reuse Linux acceptance CLI separately. Sign and run native install/start checks. Combine platform files into signed `latest.json`, `release.json`, SHA256SUMS and acceptance reports; Rust verifies every installer signature/hash.
5. Upload a draft GitHub Release, add fixed-version tags to the same image digest, then publish.
6. Advance Docker/GitHub latest only if no higher stable version exists; finally recheck attachments/digest.

Fixed versions never overwrite published images. GitHub/Docker Hub have no cross-service atomic transaction: failures may leave candidates, drafts or fixed image tags. Use **Re-run failed jobs** after repair, reusing original artifacts/digest. Different bytes for already-published attachments are rejected; publish a new version.

Other trusted same-SHA runs may supply successfully uploaded, unexpired clients/installers. Previous-attempt reuse depends on GitHub retaining artifacts; full reruns may delete them. Prefer failed-job reruns. To test cross-run reuse, start another manual rehearsal at the same commit and retain the successful source run. Only successful build jobs from the queried current attempt qualify; PR artifacts never enter releases. Metadata mismatch/corruption fails before execution. Installer reuse pins source run/artifact ID and requires successful install/start smoke tests; verify commit, version, packaging recipe, public-key configuration and installer/signature hash/size. Old formats are not reused. Reuse skips repeated installation testing, but final Rust signature verification remains mandatory. Missing/expired/deleted artifacts rebuild; identity/recipe mismatch fails. Caches only accelerate compilation.

Clients/reports retain 14 days. Clients use low compression; native installers are not recompressed. Summaries download small `release-report` metadata/acceptance only, not complete packages. Run names distinguish CI, tagging, publication and rehearsal, with links to reused source runs.

Global release concurrency prevents simultaneous latest changes. GitHub does not guarantee FIFO pending jobs; do not push multiple queued release tags in succession. Manually rerun a replaced pending tag workflow.

Immutable Releases lock attachments/tags after publication. Upload all files while draft; compare remote names/sizes/SHA256 with local artifacts before publishing, then require `immutable=true` before latest. Stale draft attachments cause refusal; clean the draft and retry. No admin token is supplied to Actions; admins enable the setting, workflows read lock status. This does not lock Docker tags: script no-overwrite checks protect fixed versions. If supported, registry exact-version immutability may add protection, but `latest`/`buildcache` must remain mutable.

<a id="演练与排错"></a>

## Rehearsal and troubleshooting

**Actions → Build and Publish → Run workflow** on the default branch pushes a public candidate and creates `online-release`, but **does not create a GitHub Release or update version/latest tags**. Official publication starts from tag push.

For initial warmup/reuse checks, run **CI** on master with `build_release_clients`, then rehearse the same commit after success. Normal version merges prebuild automatically; ordinary master pushes do not add Release compilation. Linux desktop prebuild/AppImage packaging use Ubuntu 22.04. Master saves a separate linux-desktop cache; tags reuse same-SHA binaries or rebuild with the same recipe.

Manual **Tag Merged Release** on master takes a successful master push CI run ID for read-only eligibility checks. It never creates tags/issues App tokens; only successful CI `workflow_run` events can auto-tag.

Inspect failed jobs:

- Docker login: check PAT validity/Read-Write access to `hlmark0/wukong` and GitHub configuration.
- App token: check ID/PEM, installation, Contents/PR permissions and installation approval for added permissions.
- Missing PR CI: check release-App authorship and PR Checks. Preparation success is not full CI success.
- Disk preflight: inspect `df`; use a larger hosted runner/reduce image size, never silently downgrade/skip acceptance.
- Dependency download: retry, still checking cached SHA256.
- Existing mismatched image: publish a new version, never overwrite/delete the old one.
- Acceptance: inspect doctor/failure reports in `image-validation`; do not advance latest.

Candidates/buildcache share the public repository. Periodically clean unreferenced old candidates as capacity requires. CI lacks Delete permission and does not remove tags. Preserve official versions/digests still in use.

<a id="验收边界"></a>

## Acceptance boundary

Windows runners build the frontend-embedded production client, validate packages/tamper rejection and use isolated-process fixtures for first installation, doctor-failure recovery, successful upgrades and manual rollback. These transaction tests do not call real Docker. Linux runners perform actual Docker integration. Standard Windows runners are not users' Docker Desktop/WSL2 desktops and do not prove full end-to-end acceptance there. First/key installer changes need real desktop install/upgrade checks. Windows files currently lack Authenticode signatures.

See [online installation](../user/online-install.md). Packages exclude image tars, model keys, user configuration and challenge data. Workspace import/export is separate from CI's online distribution.

<a id="应用内更新信任与恢复"></a>

## In-app update trust and recovery

Clients read official `latest.json`/`latest.json.sig`, verify the manifest first, then inspect version/platform/digest/compatibility. Downloaded EXE/AppImage also requires signature/SHA256/size verification. Docker `latest` never selects the runtime environment. Public keys in `src/desktop/update-public.key` and Tauri configuration must match. Private keys live in Actions Secrets with an external administrator backup; losing a key cannot be fixed by silently generating another for seamless existing-client updates. Updater signing does not replace Authenticode; current installers lack OS code signing.

See [desktop updates](desktop-updates.md) for check/prepare/switch/confirm rules. Manual rehearsals sign candidates without changing official releases/stable update entry points.

<a id="已验收产物的发布恢复"></a>

## Recover publication of accepted artifacts

If a tag run completed builds, native installer checks, container acceptance and package, but publish was skipped/transiently failed, run **Recover Accepted Publication** manually on master with the original tag run ID. Invoking it authorizes publishing that existing version.

It accepts only completed same-repository tag-push runs, requires each acceptance job success, pins that attempt's online-release artifact ID and checks unmoved tag/master ancestry. Download original installers/signatures/acceptance manifests; use the original commit's publisher to validate/publish identical bytes/digest. It neither compiles, resigns nor moves/creates tags, and needs no signing private key. Different existing attachments/fixed images remain protected. Missing/expired artifacts cannot use this path.

When full reruns make source artifact IDs return 404, clients/installers fall back to rebuilding; this is not skipped verification or invalid reuse. Registry candidate images survive Actions artifact deletion and can still be reused by digest.
