<a id="documentation-maintenance"></a>

# 文档维护

[English](documentation.md) · 简体中文

<a id="scope-and-layout"></a>

## 范围与目录

英文为主。人类 Markdown 文档保留原路径作为英文，同目录提供完整中文：`README.md` / `README.zh-CN.md`，或 `docs/user/usage.md` / `docs/user/usage.zh-CN.md`。范围包括 AGENTS、开发参考、组件 README 和历史叙述。双方有互跳链接，普通文档链接保持当前语言。对应显式锚点保留翻译标题和原有书签的跳转。

`README.en.md`、`docs/en/user/*.md` 只作兼容导航，不维护独立副本或缩略翻译，不再新增另一套语言目录。检查器枚举已跟踪及未忽略的新 Markdown，新增指南也须配对。

运行时 `prompts/*.md` 是执行指令资产，保持单一正文。生成报告按导出语言显示；原始日志、证据、配置示例、标识及法律声明保持原文，不作为双份人类指南。翻译解释，不翻译命令名、schema 键、地址或证据字节。历史日期、数量和限制仍属于当时；不改写原始证据来声称新验收。

<a id="update-and-review"></a>

## 更新与核对

1. 对照代码、测试和需求确认行为，先比较现有两版，避免遗漏任一版独有信息。
2. 同一 commit 更新两份。前提、步骤、选项、限制、失败情况、信任边界和验证声明一致。例子可翻译用户正文，但命令/schema 语义不变。
3. 人工逐节核对技术值和内容。标题与哈希一致**不证明**语义一致。
4. 运行 `python -m scripts.ci.check_docs`，修复配对、语言链接、本地路径/锚点和章节不匹配。
5. 核对变更后运行 `python -m scripts.ci.check_docs --record` 记录当前内容，审查 `docs/translations.json` diff，再运行检查和 `git diff --check`，一并提交两版及核对记录。

记录保存每对规范化 UTF-8 文本哈希。修改、新增或删除配对后，核对并记录前检查失败；`--record` 是人工确认记录，不是机器翻译或自动语义审核。写入前先检查结构，不能使不完整配对通过。CI 对包括纯文档 PR 在内的提交执行相同只读检查。离线检查不访问外部网站；变更的外部资料内容重要时需另行验证。

<a id="generated-catalogs"></a>

## 生成目录

工具与 Skill 清单从注册元数据生成。默认英文且不随系统语言改变；显式 `--locale zh-CN` 生成中文 Markdown。`--json` 在两种语言下完全相同。命令不打开工作区配置/状态、不连接模型。

```sh
cargo run -- tool-catalog > docs/reference/tool-catalog.md
cargo run -- --locale zh-CN tool-catalog > docs/reference/tool-catalog.zh-CN.md
cargo run -- skill-catalog > docs/reference/skill-coverage.md
cargo run -- --locale zh-CN skill-catalog > docs/reference/skill-coverage.zh-CN.md
cargo test --test tool_catalog
```

输出重定向使用 UTF-8（PowerShell 7 或明确 UTF-8 的写入方式）。中文工具说明在 `locales/zh-CN/tool-catalog.json`，绑定精确注册表源文。工具新增、删除或源文变化时，重新核对前中文生成失败。翻译仅用于人类目录，不改变模型 schema、权限或执行。重新生成两版、核对说明并更新文档记录。Skill 来源链接选择对应中文指南，工具 ID、角色、夹具 ID 和协议元数据保持规范值。
