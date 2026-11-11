# 提交信息与发布描述

[English](commits.md) · 简体中文

## 提交信息

采用 `type(scope): describe the concrete change`，模块范围可选，描述以英文为主，标题最多 100 个字符。允许的类型为 `feat`、`fix`、`perf`、`refactor`、`docs`、`test`、`build`、`ci`、`chore`、`revert` 和 `release`。机器规则位于 [commitlint.config.mjs](../../commitlint.config.mjs)；范围使用现有模块名，例如 `ui`、`updater`、`worker`。每个独立功能或修复分别提交，不要求 squash 合并。

描述行为、对象或结果，而不只是操作。`fix(updater): locate Docker credential helpers on Windows` 提供了具体信息；`fix: fix bug`、`chore: update`、`feat: improvements` 会被拒绝。较大修改在正文中说明触发条件、原因和结果，记录实际验证与限制。不兼容修改使用 `!`，并通过 `BREAKING CHANGE:` 页脚说明影响和迁移方式。格式检查无法证明内容真实、英文质量或中英文语义一致，仍需评审者结合 diff 核对。

## 本地提示与 CI

执行 `npm ci` 后，使用 `npm run lint:message -- --edit .git/COMMIT_EDITMSG` 检查信息，worktree 中应使用实际的信息文件路径。可选启用本检出的模板和 hook：

```sh
git config --local commit.template .gitmessage
git config --local core.hooksPath .githooks
```

克隆 hook 文件不会自动启用，而且本地 hook 可以绕过，最终以 CI 为准。未启用 worktree 专属配置时，Git 配置在多个 worktree 之间共享。替换他人的 hook 前需检查已有设置。如果在此设置了上述选项，可通过 `git config --local --unset commit.template` / `git config --local --unset core.hooksPath` 撤销。

`Commit policy / commit-policy` 在 PR 创建、重新打开、新提交、标题或正文编辑时检查 PR 标题、所有新增的非合并提交和发布片段。需将 GitHub Actions 的 `commit-policy` 状态检查加入 master Ruleset 的必需检查；仅提交 workflow 文件不会强制限制合并。该流程独立于较重的平台 CI，因此修改描述不会重跑构建。真实合并提交按拓扑排除，不信任标题中的声明。不重写旧历史，也不追溯检查旧提交。新的回滚提交同样使用 `revert: ...`。

兼容已有 `release: prepare vX.Y.Z` 自动化。版本 PR 使用匹配的 `codex/release-vX.Y.Z` 分支时，无需用户说明。Dependabot 的 `build(deps):` / `chore(deps):` PR（包括 `deps-dev`）无需片段，依赖详情会进入技术变更列表，但提交信息仍接受相同检查。其他机器人不整体豁免。

## 经评审的用户说明

使用英文 PR 模板，不在 PR 正文重复翻译。面向用户的修改随实现提交一个 `.changes/<unique-slug>.toml` 文件，在 PR 中引用路径，作为经评审的用户说明的唯一来源：

```toml
kind = "feature"
en = "Show live tool progress while an analysis is running."
zh_CN = "分析运行时展示工具的实时进度。"
```

允许的类别为 `feature`、`fix`、`performance`、`security` 和 `breaking`。每种语言最多 2000 个字符。不兼容修改需在两种描述中说明迁移步骤。解析器检查语言字段和明显占位符，无法验证翻译准确性或真实性；维护者需结合 diff 核对语义一致性。片段只追加：新增修正片段，不修改或删除已合入的片段。上一个稳定 tag 之后新增的片段进入下一版发布说明，普通功能 PR 无需清理片段或修改版本。

没有片段的纯内部修改，在 PR 正文添加明确的英文原因，至少 20 个字符：

```text
<!-- release-note:none: Internal naming cleanup with no user-visible change. -->
```

不可将该标记与公开片段混用。评审者判断内部修改声明是否适当。既有历史改动无需补写片段，仍在技术变更列表中展示。版本 PR 继续只修改原有五个版本文件。
