<a id="github-维护功能"></a>

# GitHub maintenance

English · [简体中文](github-maintenance.zh-CN.md)

<a id="master-merge-policy"></a>

## Main-branch merge rules

`master` uses two rulesets with different audiences. Keep CI and review exemptions separate so administrator review privileges cannot bypass execution checks.

| Ruleset | Applies to | Requirements |
| --- | --- | --- |
| `master - PR and required checks` | Everyone, with no bypass actors | PR required; GitHub Actions `summary` and `commit-policy` must pass; branch must be up to date; review conversations must be resolved; no force-push or deletion; merge/squash allowed |
| `master - review for non-admins` | Everyone except the repository administrator role | At least one approving review; dismiss stale approvals after new commits; no additional Code Owner, last-pusher or unattributed-change approval requirements |

The review-only ruleset exempts the repository administrator role using `bypass_mode: exempt`. GitHub documents this as skipping enforcement for the selected actor, unlike an explicit bypass operation. The common ruleset has no exemptions. Non-admins retain the required review; this configuration does not grant merge/write permissions to other users. See [GitHub's exemption description](https://github.blog/changelog/2025-09-10-github-ruleset-exemptions-and-repository-insights-updates/).

Before an authorized merge, read the effective branch rules and each matching ruleset's `bypass_actors` and `current_user_can_bypass`. Confirm the PR's exact head SHA, latest base, mergeability, resolved conversations and successful mandatory checks from GitHub Actions. Do not interpret `REVIEW_REQUIRED` alone as proof that the current actor lacks a permitted merge path.

GitHub auto-merge waits for merge requirements; queuing it is not a completed merge. If an administrator's ordinary, already-authorized PR remains review-blocked despite the applicable exemption, use `PUT /repos/{owner}/{repo}/pulls/{number}/merge` with the checked `sha` and `merge_method: merge`. GitHub still enforces the common ruleset. Reconcile an uncertain response by reading the PR before any retry. Do not weaken rules, fabricate approval or bypass CI to clear a blocker. A non-admin without the required approval must obtain a valid review. See the [merge API](https://docs.github.com/en/rest/pulls/pulls#merge-a-pull-request).

After either path, verify `merged=true`, record the actual `master` commit and follow triggered CI to its final outcome. Changing rules requires explicit authorization and a saved before/after configuration. Version PRs still require explicit release authorization before enabling automatic merge or merging; review exemption does not authorize publication.

<a id="actions-entry-points"></a>

## Actions entry points

The repository maintains nine workflows: six operator entry points and three internal workflows. GitHub-managed Dependabot Updates and Dependency Graph remain enabled; historical runs are retained.

| Entry | File | Responsibility and trigger |
| --- | --- | --- |
| Checks | `ci.yml` | PR/master change checks; manual full checks/client warmup; required `summary` gate |
| Acceptance | `acceptance.yml` | Manual `all`, `apt`, `uninstall`, `windows-worker` suites; Checks reuses selected APT/uninstall jobs |
| Version | `prepare-release.yml` | Preview with default `dry_run=true`; uncheck to create the five-file version PR |
| Release | `release.yml` | Default `rehearse`, read-only `inspect`, approved version-merge `tag`, accepted-publication `recover`; tag pushes publish |
| Workbench image | `workbench-image.yml` | Manual version-pinned build of the separate `<namespace>/wukong-workbench` tool image used by `cargo package-local`; pushes `local-<version>` only |
| Maintenance | `maintenance.yml` | Separate `cleanup`, `cache`, `toolchain` tasks; only cache deletion has Actions write permission |
| Internal / Policy | `commit-policy.yml` | PR title/body edits rerun policy/previews only; required `commit-policy` gate |
| Internal / Client | `build-client.yml` | Callable client build |
| Internal / Installer | `build-installer.yml` | Callable installer packaging/acceptance |

The Workbench image workflow reuses the Release Docker Hub namespace/username variables and `DOCKERHUB_TOKEN`, pushes only the separate `wukong-workbench` repository and never the official `wukong` image or its `latest` tag. A maintainer dispatches it with the client version (which must equal the checkout's `Cargo.toml` version) and an optional ref already contained in `master`; unmerged source is rejected. The `workbench-publication` Environment requires a designated maintainer's approval but has no actor allowlist. Before pushing, the workflow verifies locked inputs, runs real sibling-container acceptance and fails closed on registry inspection errors other than a confirmed missing tag. After pushing, it verifies the remote digest and records actor, source revision, digest and size. Creating the `wukong-workbench` repository once is the only manual Docker Hub setup; see [local-release builds](local-builds.md#ci-built-workbench-image).

`Acceptance/all` runs all three specialized suites; it does not replace full component Checks or Release/rehearse container/package acceptance. Native installation/uninstallation runs only on disposable hosted runners. Internal workflows can still appear in Actions, distinguished by name. Ordinary PRs have only Checks and Policy as top-level runs; selected acceptance jobs report through Checks. Ordinary master changes do not start Release.

Explicitly enable GitHub native auto-merge after creating an ordinary PR intended for automatic integration; allowing auto-merge at repository level does not enable it for every PR. Identify whether CI, review or conversation resolution is pending, retaining administrator review exemption and at least one approval for non-admins.

## Dependabot

Default-branch `.github/dependabot.yml` manages Cargo, the npm workspace, `workers/runtime/requirements.txt` and commit-pinned GitHub Actions. Checks are staggered Mondays 09:10–09:40 Beijing time, with at most seven simultaneous version-update PRs. Rig dependencies update together; other compatible updates group by ecosystem, with separate security groups. Repository settings enable alerts/automatic security updates. PRs still require CI and maintainer merge.

Inline Dockerfile Python dependencies, apt/gem, AFL base images and custom tool locks are outside this configuration. Rust compiler updates must change `rust-toolchain.toml` and Cargo `rust-version` together. Check compatibility between Tauri Rust/npm packages.

Dependabot uses ordinary CI without Docker Hub/model secrets. Its configuration does not govern other PR merge methods or replace plan-restricted private-repository Rulesets.

<a id="工具链更新检查"></a>

## Toolchain update checks

Run **Actions → Maintenance → Run workflow (`task=toolchain`)**, or wait for Mondays 10:23 Beijing time (GitHub schedules can be delayed). Standard Ubuntu runners use read-only permissions, no Docker Hub/model secrets and no large installer downloads.

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

Native Windows worker installation acceptance is opt-in: enable `windows_worker_acceptance` on a manual Checks run, or add the `windows-worker-acceptance` PR label before pushing the revision to test. A label edit alone does not start CI. The option forces full component checks and runs the dedicated hosted fixture after native compilation; remove the label after the selected run starts to avoid repeating installation for later unrelated revisions. The fixture rejects developer/self-hosted machines and uploads only its public report. Worker executable external-network blocking keeps the CI agent online and does not replace the final disconnected Windows VM acceptance. It neither publishes a release nor proves production target isolation/uninstall; see the [native component guide](../../workers/windows/README.md).

Native compilation and selected installation acceptance use an explicit non-cancellation condition, so an unrelated earlier test failure does not suppress this independent evidence. Installation still requires the native build/check step to pass; cancellation, a failed native build or an unselected fixture prevents installation. Reports upload only when the fixture actually ran to success/failure. Earlier failures still fail the Windows job and final CI gate; no step uses error continuation. See GitHub's [status-check expression rules](https://docs.github.com/en/actions/reference/workflows-and-actions/expressions#status-check-functions).

`scripts/ci/ci_scope.py` scopes both PR and master checks: README/AGENTS/docs Markdown, valid append-only `.changes/<slug>.toml` fragments and translation records are lightweight, skipping entire Windows/Linux jobs; changes always validates documentation pairs, links and review fingerprints. UI adds frontend checks on both Windows and Linux; runtime/tests/fixtures/evals add Linux and Windows Python tests. Rust/shared scripts/locks/workflows/unknown paths run full component checks; runtime prompts are executable assets. Version PRs, master version increases, manual Checks and release calls run full components plus APT/uninstall acceptance. PRs use the merge-base range; pushes use the entire before/after range. Renames include both paths; scope failure blocks CI. Windows release-script/installer transaction tests run with Rust/shared changes only.

`summary` is the final gate: changes must succeed, and Windows, Linux, reusable acceptance and all three client prebuilds must match the selected scope. APT/uninstall are selected by affected paths or workflow changes; the child acceptance-gate independently checks each selected job. Failures, cancellations and unexpected skips cannot pass; only unselected tasks may skip. Always trigger top-level Checks and select jobs inside it, avoiding permanently pending required checks. Policy title/body edits do not cancel platform CI.

Rust cache keys include toolchain/options. Master writes; PRs restore. `windows-checks-v2` is separate from Release compilation. Cold/toolchain/option changes rebuild. `windows-client-v1`/`linux-client-v1` are written by master version prebuilds or explicit warmups; tags restore, avoiding caches trapped in tag scope. Summaries report exact hits and restore/save/fmt/Clippy/test durations. Non-exact hits can mean fallback or miss; see logs for the restored key. Clippy/tests share a Windows build directory rather than duplicate dependencies for parallelism. Windows CI sets `CARGO_PROFILE_DEV_DEBUG=1`, `CARGO_PROFILE_TEST_DEBUG=1`, retaining debug assertions/overflow checks with less type/variable debugging information. Local/release profiles stay unchanged. These settings participate in keys, so the first changed-profile run cannot reuse the full old cache.

Compare cold, same-configuration cache-hit and Python-only PR timings separately, including total waiting and runner use. Do not present cold-vs-warm as equal-condition speed comparisons.

Ubuntu dependency steps in CI, client builds and installer builds share `python -m scripts.ci.install_apt <packages>`. The helper refuses developer/self-hosted machines and supports only disposable GitHub-hosted Ubuntu 22.04/24.04 x64 runners. It checks installed package status first, downloads only missing packages and probes the requested tools/libraries afterward. If all packages are already installed, it skips network access and leaves APT configuration unchanged. Linux worker checks require debugging/fixture compilers; desktop development libraries are added only for Rust desktop checks, in the same installation step.

When downloads are needed, the helper tries the HTTPS Ubuntu mirrors at kernel.org, MIT and finally Canonical archive/security as whole attempts. Each attempt uses direct signed sources and fresh isolated indexes. An index or package download failure abandons that mirror for the rest of the invocation; subsequent files do not return to it. The selected configuration remains active for indirect APT callers such as Playwright. Original source files, DNS, credentials and Ubuntu archive-keyring signature checks are preserved. Index updates have a 40-second limit per mirror, package downloads 120 seconds, and all network attempts share a 480-second budget. APT transfer timeouts are ten seconds with no per-file retries; process termination has a five-second grace period. Only complete downloads permit the separate `--no-download` local installation phase, limited to 180 seconds. Translation and command-not-found indexes are omitted. Dependency steps retain a twelve-minute outer limit; the frontend step retains fifteen minutes. All-mirror failure or failed package/tool checks fail CI rather than accepting stale indexes or skipping required tests. This bounds failures without guaranteeing mirror/network availability; product/runtime and Docker image APT configuration are unaffected.

`Acceptance / apt` is selected by helper/acceptance changes or workflow changes, or manually with `suite=apt`. Its Ubuntu 22.04/24.04 matrix exercises actual signed indexes and package downloads, separate offline installation and preinstalled-package reuse. Loopback servers accept connections but withhold headers, or serve the original signed InRelease and stall partway through an index body. Both must abandon the complete failed mirror and recover through an official source. A sole stalled mirror must fail within its deadline despite earlier valid indexes. The fixture checks inherited APT configuration and unchanged original source files. This does not build/publish installers or replace platform CI, Docker acceptance or native uninstall tests.

CI/Release summary jobs run after dependencies, including failure. Run pages show commit/ref, status, duration and artifact links; Release adds version, digest, unpacked image/package size and actual container acceptance. Manual rehearsals explicitly say unpublished; tag runs link official releases only after publish succeeds.

Statuses come from `needs`; durations/artifacts use read-only Actions API. If unavailable, show missing information, not zero duration or assumed acceptance. Forced cancellation/GitHub scheduling issues may prevent summary execution; raw logs remain authoritative. Toolchain reports retain 30 days; Release packages/acceptance reports retain 14.

<a id="docker-与附件缓存"></a>

### Docker and dependency caches

`Maintenance / cache` warms master on lock/downloader/workflow changes and supports manual runs. Keys include tool-lock hash. Release restores master attachments and verifies every SHA256. Downloads default to at most three parallel assets; `--jobs 1` is serial. Verified temporary files replace final files; network errors/corruption cannot be cache inputs. Eviction causes redownload.

Workbench inherits a stable toolchain stage; runtime/prompts/version labels come after tool installation. Tools mount only their own assets, avoiding invalidating all layers when a shared directory changes. Docker Hub `buildcache` stores complete intermediate layers. Pip cache mounts help within one builder; cross-runner persistence comes from registry layers/attachment caches, not assumed shared cache mounts.

<a id="关闭-pr-后的缓存回收"></a>

### Closed-PR cache cleanup

`Maintenance / cleanup` runs daily at 06:41 Beijing time, rechecks current PR state and deletes by cache ID: closed PR `refs/pull/N/merge` caches and corresponding closed Dependabot branch caches. Master/tags/open PRs/unknown ordinary branches remain. Scheduling may be delayed. Manual `task=cleanup` only lists candidates unless `apply` is selected. Daily cleanup and weekly toolchain reporting route by their own cron; neither triggers the other. Summaries show count/reclaimed bytes. It runs only on master, never `pull_request_target` or PR code, and needs no personal admin token.

## Commit policy and release previews

The read-only `Internal / Policy` workflow validates PR titles/new non-merge commits with pinned commitlint and checks release fragments. PRs require English only. It reruns on PR title/body edits without restarting platform CI, and runs real git-cliff/commitlint regression tests before generating PR-only/cumulative previews. See [commit rules](commits.md) for optional local hooks and bilingual release fragments. Require the GitHub Actions `commit-policy` context alongside `summary` in the common master ruleset. Neither context substitutes for the other; review exemptions follow the [main-branch merge policy](#master-merge-policy), while conversation resolution and strict updates remain required for everyone. A workflow file alone cannot enable a server-side merge requirement. No release authorization or tag is implied by this policy check.
