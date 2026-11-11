# 维护与性能检查

## 模块边界

- `kernel` 只装配 Rig 应用和驱动生命周期。`admission` 管理准入，`models` 管理模型请求与计量，`coordination` 管理消息/状态投影，`business_tools` 接入工具效果。`kernel_host` 暴露业务操作，`supervisor` 独立管理心跳和后台任务观察；`model_catalog`、`usage`、`bootstrap` 分别负责模型选择、用量和装配。
- `tools` 保留执行管线和共同校验入口，`effects` 管理持久化回执；`catalog` 统一注册 schema、角色、恢复策略与处理器，`handlers` 执行业务路由，`coordination` 处理共享资料与任务，`analysis` 处理静态查询，`execution` 分派 worker 请求，`verification` 统一验证准入与认证，`files`、`jobs`、`sessions` 各自管理资源。参数及 handlers 在各自子目录按职责分组，仍由单一 catalog 注册。
- `queries` 定义共享查询与 DTO，`store/workspace`、`store/report_queries` 实现 SQL；`desktop/workspace` 通过 `store/desktop_queries` 读取桌面快照，`desktop/presentation` 适配公开进度和工具结果，React 界面消费 DTO。`ui/src/commands.ts` 约束命令参数/响应，生命周期副作用放在对应 hook 中。
- `coordinator` 按 runs、tasks、messages、waits、completion 划分用例，继续通过同一 Store 事务提交。`binary` 保留公共地址/哈希接口，画像、字符串及 Ghidra 生命周期在独立子模块中维护。
- `store` 对业务修改提供 IMMEDIATE 事务，对查询提供独立只读 WAL 快照。`store/conversations` 负责独立会话载荷及其修订号。当前状态 schema 为 3，不提供旧数据库迁移。

worker 组件划分见 `workers/runtime/README.md`。部署整个运行时目录，入口文件不承担模块重导出。

跨模块调用保持类型化，资源归属和业务完成门槛保留在统一入口。通用元数据查询不读取 checkpoint；需要继承、follow-up 或完整导出时显式使用完整读取接口。元数据写入不得清空会话，checkpoint 与邮箱确认仍在同一持久化事务提交。

## 桌面界面样式

界面、原生消息和报告的双语资源及验收要求见[多语言维护](localization.md)。用户语言偏好不得通过工作区配置保存流程修改。

`ui/src/styles.css` 只按固定顺序导入 `styles/` 中的基础、工作区、聊天、Agent、证据、设置、响应式及维护界面样式；移动规则时保持层叠顺序。
前端使用现有的 Radix、assistant-ui 和 Lucide 组件。颜色统一定义在 `ui/src/theme.css`，组件样式引用语义变量；不要在组件中重新散布颜色值。
`ui/src/workspace-layout.css` 只放分栏容器和拖动分隔条；组件的尺寸、间距、边框直接归入 `styles/` 对应文件，响应式规则统一放在 `styles/responsive.css`，不叠加一层旧布局覆盖。分栏使用 `react-resizable-panels`，与 shadcn/ui 的分栏采用同一基础组件，保留键盘调整和宽度记忆。
桌面视图偏好通过 `useViewPreference` 保存为本机 WebView 数据；任务别名按工作区路径和 run ID 区分，不能写回任务指令或作为目标身份。设置与存储草稿留在设置组件状态中，目录操作仍经过既有 epoch 与维护锁检查。

默认采用中性深灰底色，金色表示主要操作或选中，蓝色表示执行中，绿色表示成功，黄色表示等待/警告，红色表示失败。状态同时保留图标和文字。
正文使用 14px，辅助信息至少 12px，代码与原始证据使用 13px 等宽字体。调整布局时检查桌面最小窗口下的滚动、长路径和消息草稿保留。
颜色层级参考 [Radix Colors](https://www.radix-ui.com/colors/docs/palette-composition/understanding-the-scale)，工作区分区参考 [VS Code](https://code.visualstudio.com/docs/editing/getting-started/userinterface)。

## 工具文档

工具定义、参数 schema、角色权限、模型可见性和显示消息 ID 来自同一注册表；双语文案来自共享 Fluent 资源。内部入口由 `Visibility::Internal` 标记，不通过名称黑名单逐层过滤；内部事务调用保留权限校验。桌面只为原生 Rig 工具和旧事件名称保留 `desktop/tool_aliases` 映射，新增内置工具必须在 catalog 提供消息 ID 并补齐双语标题。Markdown 工具清单也从注册表生成：

```sh
cargo run -- tool-catalog > docs/reference/tool-catalog.md
cargo run -- --json tool-catalog
cargo test --test tool_catalog
```

导出目录无需模型凭据，也不会打开配置或状态目录。测试比较生成结果与已提交文档，防止工具改动后遗漏更新。

## Skill 覆盖清单

内置方法的工具、来源及机制夹具关联记录在 `prompts/coverage.json`，复用 `SkillManifest` 的 `related_tools`、`sources`、`fixtures` 字段。更新后生成 [覆盖清单](../reference/skill-coverage.md)：

```sh
cargo run -- skill-catalog > docs/reference/skill-coverage.md
cargo run -- --json skill-catalog
cargo test --test tool_catalog
```

这些字段只用于维护和查询，不授予工具权限，也不证明当前后端可用、原题复现或 Agent 解题成功率。未登记表示没有关联记录。自定义 Skill 可在原有 manifest 中填写这些字段；覆盖内置正文时不会自动继承内置夹具声明。单元测试校验内置关联的工具、来源文件及夹具 ID 存在，导出不读取模型凭据或状态目录。

## 共享校验与内容身份

`domain/claims` 统一解析 flag 形状的声明；完成工具拒绝占位符，协调器按具体声明执行独立验证门槛。解析结果只表示文本分类，不证明利用成功。

Rust 文件哈希和实际读取字节计数放在 `content`，`binary` 保留公开哈希接口；worker 使用 `worker_common.sha256_file`，构建与发布脚本使用 `scripts._lib.checksums.sha256`。独立注入沙箱的脚本保持自包含，不引入控制端模块。路径限制、大小预算和控制端/worker 的独立身份核对仍留在调用边界，不因共用哈希函数而合并或删除。

## 验证

```sh
cargo fmt --all -- --check
cargo clippy --offline --all-targets -- -D warnings
cargo test --offline --all-targets
python workers/tests/run.py unit -p test_bridge.py
```

运行 worker 测试前，在虚拟环境中安装 `workers/runtime/requirements.txt` 并确保 `python` 命令可用。普通测试使用可信本地夹具和回环服务；被忽略的真实 Ghidra/Docker 验收需要按各测试说明配置环境。

## 可重复性能基准

```sh
cargo test --offline --lib workspace_history_performance_baseline -- --ignored --nocapture
```

基准在临时目录分别生成 1000、10000 和 100000 条事件，输出首次读取耗时和 100 次空闲读取的平均耗时；每档只加载最新 500 条事件，空闲读取不返回重复的历史和 Agent 列表。输出为实测值，不设置依赖硬件速度的断言。

2026-11-08 本地基准（Darwin arm64、Rust 1.99.1、未优化 test 构建；首次读取单次，空闲读取取 100 次平均）：

| 历史事件数 | 首次读取 / ms | 空闲读取平均 / ms | 加载事件数 |
|---:|---:|---:|---:|
| 1000 | 3.769 | 0.135 | 500 |
| 10000 | 3.723 | 0.139 | 500 |
| 100000 | 3.761 | 0.134 | 500 |

这些数值用于后续同环境比较，不代表界面端到端帧耗时，也未与改动前版本作速度对比。

前端使用虚拟列表限制聊天 DOM 数量；运行 `npm test`、`npm run build` 和 `npm run test:e2e` 验证类型、组件、任务切换、草稿、关闭确认、文件拖入与最小窗口交互。浏览器测试通过 Tauri 模拟边界运行，不代表真实桌面安装验收。存储测试验证有写事务时仍可读取已提交快照，元数据更新与重复 checkpoint 不改写会话。

评估后续改动时记录数据规模、构建模式和硬件环境，并分别观察界面帧耗时、查询等待、内存和数据库写入量。审计记录、证据与恢复边界的持久化不可因 UI 降频而丢失。
