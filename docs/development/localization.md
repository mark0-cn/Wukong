# 多语言维护

## 资源与边界

Wukong 的界面支持 `zh-CN` 和 `en-US`。`locales/<locale>/*.ftl` 是前端和 Rust 共用的 Fluent 资源；Vite 与 `build.rs` 将其嵌入客户端。运行时不请求翻译服务，也不下载语言包。

- `common`：应用标题、语言设置和原生对话框。
- `workspace`、`settings`、`updates`：桌面界面。
- `tools`：catalog 及历史工具别名对应的显示消息。
- `errors`：控制端产生的状态、通知和可预期错误。
- `cli`、`report`：命令行帮助和可读导出。

工具名称、JSON 字段、状态枚举、路径、地址、payload、内容哈希和原始证据不翻译。工具返回的原始文本、已有模型正文和第三方错误也保持原样；应用生成的错误使用 `{code,args,detail}`，`detail` 保留原始诊断。不根据错误正文或中文子串推测错误类型。

React 通过语言订阅更新组件；语言不进入组件 key，也不触发 workspace config save、epoch 变化或任务重启。不要在模块初始化时缓存翻译结果；状态标签和设置分组应在渲染时求值。完整句子使用 Fluent 参数，避免把语序拆成多个翻译片段。

## 偏好与任务输出

`preferences.json` 保存在固定用户数据入口（见[在线安装](../user/online-install.md)），与模型配置、任务数据库、可切换缓存目录独立。原生命令用同目录临时文件原子替换；前端只有保存成功才应用新偏好。

界面默认跟随系统。`zh`、简体中文及新加坡中文映射到 `zh-CN`，其他语言回退 `en-US`。语言设置修改不会清空消息、设置或任务草稿。

新任务输出可选中文、英文或自动。自动模式是有界默认规则：初始任务含汉字时选中文，含 ASCII 字母时选英文，其他输入使用当前界面语言（CLI 使用系统语言）。它不调用额外模型，不承诺准确识别混合语言；需要确定结果时显式选择语言。应用将具体值存入任务配置的 `output_language`。恢复读取原快照，已有任务不跟随界面或新任务偏好改变。缺失该字段的历史任务保留原来的“跟随用户语言，未指定时中文”提示策略。

`agents::preamble` 在稳定的角色、工具与 Skill 前缀之后注入统一输出策略。用户在任务中明确要求的输出语言优先。Skill 方法正文保持一份，不因界面语言复制方法或权限定义。提示词测试验证策略注入及恢复不变量，不代表真实模型必然遵循；当前测试不产生付费模型请求。

## CLI、报告和安装器

- `--locale auto|en-US|zh-CN` 控制 CLI 帮助、应用错误包装和可读报告，默认读取 `WUKONG_LOCALE`，命令行优先。
- `--output-language en-US|zh-CN`（或 `WUKONG_OUTPUT_LANGUAGE`）控制新任务输出；配置中的 `output_language` 也可设置。恢复任务忽略新的默认值。
- 命令及参数名保持稳定；Clap 的原始解析诊断和技术元数据可能仍为英文。结构化 JSON 和规范工具/Skill 目录不按显示语言改写。
- 桌面导出使用导出时的界面语言；CLI 使用 `--locale`。报告标题、状态与表头翻译，已有正文和证据不重写。导出测试比较两种语言的快照、重放数据和原始字节。
- Windows NSIS 提供 English/SimpChinese 选择；安装器语言和应用偏好相互独立。Linux AppImage 的应用内准备/恢复提示跟随界面，桌面入口提供中文 `Comment`。两种语言共用安装文件、版本、签名、更新清单和工具镜像。

## 新增或修改翻译

1. 在两个目录的对应资源中添加稳定、可读的消息 ID；变量名和数量保持一致。不要把用户内容用作消息 ID。
2. 执行 `node scripts/check-locales.mjs --write` 更新前端 `Messages` 类型。
3. 界面使用 `t`，原生边界使用 `i18n::Message`。只有界面渲染时求值，完整原始数据继续保存在原位置。
4. 执行 `npm run check:locales`、`npm test`、`npm run build` 和相关 Rust 测试。交互改动执行 `npm run test:e2e`。

资源检查在 npm 的测试、类型检查和构建之前自动运行，拒绝语法错误、重复 ID、键或变量不一致、过期的生成类型。工具目录测试核对每个工具的双语标题。CI 在 Windows/Linux 运行桌面特性和双语浏览器回归。

浏览器测试使用模拟的 Tauri 命令边界，不能代替真实 WebView、NSIS 交互选择、Windows Docker Desktop 或跨版本升级验收。安装器相关验证按[桌面更新设计](desktop-updates.md)在隔离环境执行。
