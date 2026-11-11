<a id="github-维护功能"></a>

# GitHub maintenance

English · [简体中文](github-maintenance.zh-CN.md)

## Dependabot

Default-branch `.github/dependabot.yml` manages Cargo, the npm workspace, `workers/runtime/requirements.txt` and commit-pinned GitHub Actions. Checks are staggered Mondays 09:10–09:40 Beijing time, with at most seven simultaneous version-update PRs. Rig dependencies update together; other compatible updates group by ecosystem, with separate security groups. Repository settings enable alerts/automatic security updates. PRs still require CI and maintainer merge.

Inline Dockerfile Python dependencies, apt/gem, AFL base images and custom tool locks are outside this configuration. Rust compiler updates must change `rust-toolchain.toml` and Cargo `rust-version` together. Check compatibility between Tauri Rust/npm packages.

Dependabot uses ordinary CI without Docker Hub/model secrets. Its configuration does not govern other PR merge methods or replace plan-restricted private-repository Rulesets.

<a id="工具链更新检查"></a>

## Toolchain update checks

Run **Actions → Toolchain updates → Run workflow**, or wait for Mondays 10:23 Beijing time (GitHub schedules can be delayed). Standard Ubuntu runners use read-only permissions, no Docker Hub/model secrets and no large installer downloads.

Checks cover stable pwndbg, pwninit, Ghidra, Temurin JDK 25 and GEF. The 30-day `toolchain-update-report` artifact contains `report.md`, `report.json`, `candidates.json`, including differences, upstream links and SHA256 sources. New versions are normal; API/asset format errors or changed same-version SHA256 fail. Assets without upstream SHA256 are `checksum_review_required` and omitted from candidates. GEF pins source commit, verifies Git blob identity and computes SHA256, requiring source review; this is not an independent publisher signature/checksum.

Checks propose upgrades only: no automatic lockfile edits, PRs, merges or releases. Review candidates/upstream changes, update locks and duplicated Dockerfile SHA256/paths, verify downloads, rebuild and run full acceptance. Temurin stays on JDK 25; maintain apt/gem/inline pip/AFL separately.

<a id="不可变-release"></a>

## Immutable Releases

Repository **Settings → General → Releases → Enable release immutability** is enabled for subsequent releases. Publication locks attachments/associated tags; upgrades require new versions. Titles, notes and latest markers may still change. Never delete official releases to reuse tags.

The publisher verifies every draft attachment name/size/SHA256 before publication and checks `immutable=true` before advancing latest. If an administrator disables immutability, publication checks fail rather than silently accept mutable releases. Ordinary workflows need no admin permissions.

<a id="actions-运行摘要"></a>

## Actions summaries

<a id="ci-调度与缓存"></a>

### CI scheduling and caches

Ordinary CI triggers on `master` pushes and PRs targeting `master`, avoiding duplicate branch push/PR runs. Unopened branches can run manually. Version preparation uses the release App to create PRs, triggering native PR CI. New PR revisions cancel old runs; master runs finish to save caches, while GitHub concurrency coalesces pending work.

`scripts/ci/ci_scope.py` selects PR checks: README/docs-only Markdown, append-only `.changes/<slug>.toml` release fragments and the paired translation review record get lightweight checks; the changes job always validates documentation pairs, links and review fingerprints; `ui/` adds frontend; runtime/tests/fixtures/evals add Linux tests. Rust, common scripts, locks, workflows, unknown paths and version PRs get full checks. Renames inspect both paths; scope-computation failure blocks CI. Windows version/release-script/installer checks always run. Master/manual/release calls are always full.

`summary` is the final gate: scope/Windows must succeed; Linux status must match selected scope; selected release-client prebuilds must also succeed. Require `summary`, not only conditionally skipped `linux`. Expected skips are explicit; failures, cancellations or unexpected skips cannot pass.

Rust cache keys include toolchain/options. Master writes; PRs restore. `windows-checks-v2` is separate from Release compilation. Cold/toolchain/option changes rebuild. `windows-client-v1`/`linux-client-v1` are written by master version prebuilds or explicit warmups; tags restore, avoiding caches trapped in tag scope. Summaries report exact hits and restore/save/fmt/Clippy/test durations. Non-exact hits can mean fallback or miss; see logs for the restored key. Clippy/tests share a Windows build directory rather than duplicate dependencies for parallelism. Windows CI sets `CARGO_PROFILE_DEV_DEBUG=1`, `CARGO_PROFILE_TEST_DEBUG=1`, retaining debug assertions/overflow checks with less type/variable debugging information. Local/release profiles stay unchanged. These settings participate in keys, so the first changed-profile run cannot reuse the full old cache.

Compare cold, same-configuration cache-hit and Python-only PR timings separately, including total waiting and runner use. Do not present cold-vs-warm as equal-condition speed comparisons.

Ubuntu dependency steps in CI, client builds and installer builds share `python -m scripts.ci.install_apt <packages>`. It refuses developer/self-hosted machines and supports only disposable GitHub-hosted Ubuntu 22.04/24.04 x64 runners. A job-local APT configuration selects signed Ubuntu archive/security repositories with the installed Ubuntu archive keyring, isolated indexes and unchanged signature checks. APT's built-in `mirror+file` transport tries the HTTPS Ubuntu mirrors at kernel.org and MIT, then the Canonical archive/security endpoints, failing over per file during index and package downloads. Independent mirror domains cover the observed Ubuntu-domain DNS failure. This bypasses the runner's Azure mirror selector without editing its original source files, DNS or credentials. The same configuration covers Playwright's dependency installer. Transfers have 15-second timeouts and two retries; index update/install have 120/480-second process limits plus 15 seconds to kill remaining processes. Dependency steps have a 12-minute outer limit, and the Linux frontend step has a 15-minute limit. Failed index downloads fail strictly instead of accepting stale indexes. Dependency installation is separate from tests, so network failures cannot appear as hung test execution. See the Ubuntu [APT configuration](https://manpages.ubuntu.com/manpages/jammy/man5/apt.conf.5.html), [apt-get](https://manpages.ubuntu.com/manpages/jammy/man8/apt-get.8.html) and [mirror transport](https://manpages.ubuntu.com/manpages/jammy/man1/apt-transport-mirror.1.html) manuals. This bounds failures; it cannot guarantee availability of Ubuntu or the runner network. Product/runtime and Docker image APT configuration are unaffected.

`Ubuntu dependency acceptance` runs when its helper/acceptance files change in a PR, or manually. Its small Ubuntu 22.04/24.04 matrix installs real signed packages, checks the configuration inherited by indirect APT callers proves that APT recovers from an unreachable first mirror, and checks that a sole unreachable loopback repository returns APT error 100. It also checks that the original source files are unchanged. This does not build/publish installers or replace the platform CI, Docker acceptance or native uninstall tests.

CI/Release summary jobs run after dependencies, including failure. Run pages show commit/ref, status, duration and artifact links; Release adds version, digest, unpacked image/package size and actual container acceptance. Manual rehearsals explicitly say unpublished; tag runs link official releases only after publish succeeds.

Statuses come from `needs`; durations/artifacts use read-only Actions API. If unavailable, show missing information, not zero duration or assumed acceptance. Forced cancellation/GitHub scheduling issues may prevent summary execution; raw logs remain authoritative. Toolchain reports retain 30 days; Release packages/acceptance reports retain 14.

<a id="docker-与附件缓存"></a>

### Docker and dependency caches

`Cache Worker Dependencies` warms master on lock/downloader/workflow changes and supports manual runs. Keys include tool-lock hash. Release restores master attachments and verifies every SHA256. Downloads default to at most three parallel assets; `--jobs 1` is serial. Verified temporary files replace final files; network errors/corruption cannot be cache inputs. Eviction causes redownload.

Workbench inherits a stable toolchain stage; runtime/prompts/version labels come after tool installation. Tools mount only their own assets, avoiding invalidating all layers when a shared directory changes. Docker Hub `buildcache` stores complete intermediate layers. Pip cache mounts help within one builder; cross-runner persistence comes from registry layers/attachment caches, not assumed shared cache mounts.

<a id="关闭-pr-后的缓存回收"></a>

### Closed-PR cache cleanup

`Clean Closed PR Caches` runs daily at 06:41 Beijing time, rechecks current PR state and deletes by cache ID: closed PR `refs/pull/N/merge` caches and corresponding closed Dependabot branch caches. Master/tags/open PRs/unknown ordinary branches remain. Scheduling may be delayed. Manual runs only list candidates unless `apply` is selected. Summaries show count/reclaimed bytes. It runs only on master, never `pull_request_target` or PR code, and needs no personal admin token.

## Commit policy and release previews

The read-only `Commit policy` workflow validates PR titles/new non-merge commits with pinned commitlint and checks release fragments. PRs require English only. It reruns on PR title/body edits without restarting platform CI, and runs real git-cliff/commitlint regression tests before generating PR-only/cumulative previews. See [commit rules](commits.md) for optional local hooks and bilingual release fragments. Require the GitHub Actions `commit-policy` context alongside the existing `summary` context in the master Ruleset. Neither context substitutes for the other; retain existing approval, thread-resolution and strict-update requirements. A workflow file alone cannot enable a server-side merge requirement. No release authorization or tag is implied by this policy check.
