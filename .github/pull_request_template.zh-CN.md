[English](pull_request_template.md) · 简体中文

## 修改

说明触发条件和修改后的行为，独立修复应分别提交。

## 验证

列出实际运行的检查以及重要的未覆盖范围。

## 兼容性

不兼容修改需说明迁移步骤，否则明确无需迁移。

## 发布说明

PR 标题和正文只需英文。面向用户的修改需增加双语 `.changes/<slug>.toml` 片段，并在此引用路径。参见[提交指南](../docs/development/commits.zh-CN.md)，不在 PR 中重复翻译。

纯内部修改未增加片段时，添加该指南中的英文 `release-note:none:` 原因标记。
