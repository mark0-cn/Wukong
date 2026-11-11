# Commit messages and release descriptions

English · [简体中文](commits.zh-CN.md)

## Commit messages

Use `type(scope): describe the concrete change` with an optional module scope, English descriptions and a header of at most 100 characters. Allowed types are `feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `build`, `ci`, `chore`, `revert` and `release`. The executable rules are in [commitlint.config.mjs](../../commitlint.config.mjs); use existing module names such as `ui`, `updater` or `worker` for scopes. Keep each independent feature/fix in its own commit; squash merging is not required.

Describe the behavior/object/result, not just the activity. `fix(updater): locate Docker credential helpers on Windows` is useful; `fix: fix bug`, `chore: update` and `feat: improvements` are rejected. For substantial changes, explain the trigger, reason and resulting behavior in the body. Record actual validation and limits. Mark incompatible changes with `!` and describe the impact and migration in a `BREAKING CHANGE:` footer. Formatting checks cannot prove truth, English quality or semantic equivalence; reviewers still compare descriptions with the diff.

## Local feedback and CI

After `npm ci`, `npm run lint:message -- --edit .git/COMMIT_EDITMSG` checks a message (use the actual message path in worktrees). Optionally enable this checkout's template and hooks:

```sh
git config --local commit.template .gitmessage
git config --local core.hooksPath .githooks
```

Hooks do not activate just because they are cloned and can be bypassed; CI is authoritative. Git configuration is shared by worktrees unless worktree-specific configuration is enabled. Do not replace somebody else's hooks without reviewing them. Undo these options with `git config --local --unset commit.template` / `git config --local --unset core.hooksPath` if you set them here.

`Commit policy / commit-policy` checks PR titles, all new non-merge commits and release fragments on opening, reopening, new commits and title/body edits. Add `commit-policy` as a GitHub Actions required status check in the master Ruleset; a checked-in workflow alone does not enforce merges. It is separate from expensive platform CI, so editing a description does not restart builds. Real merge commits are excluded by topology, not by trusting their messages. Old history is not rewritten or retroactively linted. New revert commits must use `revert: ...` too.

Existing `release: prepare vX.Y.Z` automation is supported. A version PR's matching `codex/release-vX.Y.Z` branch needs no user note. Dependabot PRs with `build(deps):` / `chore(deps):` (including `deps-dev`) need no fragment because dependency details appear in the technical changelog; their commit messages still pass the same checks. Other bots have no blanket exception.

## Reviewed user descriptions

Use the English PR template; do not repeat translations in the PR body. For a user-facing change, commit one `.changes/<unique-slug>.toml` file alongside the implementation and reference its path in the PR. It is the single source of reviewed user-facing descriptions:

```toml
kind = "feature"
en = "Show live tool progress while an analysis is running."
zh_CN = "分析运行时展示工具的实时进度。"
```

Allowed kinds are `feature`, `fix`, `performance`, `security` and `breaking`. Each language is limited to 2000 characters. Explain migration steps in both descriptions for breaking changes. The parser checks language fields and obvious placeholders, not translation accuracy or truth; maintainers review semantic equivalence against the diff. Fragments are append-only: add a correction rather than changing/deleting a merged fragment. New fragments since the previous stable tag belong to the next release, without cleanup or version edits in normal feature PRs.

For an internal-only change without a fragment, add an explicit English reason to the PR body (at least 20 characters):

```text
<!-- release-note:none: Internal naming cleanup with no user-visible change. -->
```

Do not combine this marker with public fragments. Reviewers decide whether the internal-only declaration is appropriate. Historical changes need no backfilled fragments and remain visible in the technical changelog. The version-only PR continues to change exactly the existing five version files.
