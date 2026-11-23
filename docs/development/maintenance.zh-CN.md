<a id="maintenance-and-performance-checks"></a>

# 维护与性能检查

[English](maintenance.md) · 简体中文

<a id="module-boundaries"></a>

## 模块边界

- `application/agent_runtime` 只装配 Rig 应用和驱动生命周期。`admission` 管理准入，`models` 管理模型请求与计量，`coordination` 管理消息/状态投影，`business_tools` 接入工具效果。`application/agent_runtime/host` 暴露业务操作，`application/assessment/supervisor` 独立管理心跳和后台任务观察；`application/model_catalog`、`application/usage`、`bootstrap` 分别负责模型选择、用量和装配。
- `application/tools` 保留执行管线和共同校验入口，`application/effects` 管理持久化回执；`catalog` 统一注册 schema、角色、恢复策略与处理器，`handlers` 执行业务路由，`coordination` 处理共享资料与任务，`analysis` 处理静态查询，`execution` 分派 worker 请求，`verification` 统一验证准入与认证，`files`、`jobs`、`sessions` 各自管理资源。参数及 handlers 在各自子目录按职责分组，仍由单一 catalog 注册。
- `queries` 定义中立 DTO。桌面、CLI 和报告模块通过 `application/queries` 中持有 Store 的查询入口读取数据。`storage/queries` 管理 SQL，并在同一事务读取完整工作区快照。桌面展示层在事务结束后转换快照，不额外查询，并分别呈现任务的执行结果、清理确认和阻塞原因。架构检查禁止 `interfaces` 直接导入 `storage/queries`。`ui/src/shared/api/commands.ts` 约束命令参数/响应，生命周期副作用放在对应 hook 中。
- `application/windows_worker_setup/inventory` 投影只读部署诊断：`application/worker_media` 管理生成记录扫描，安装状态模块管理恢复记录与已导入回执，配置管理当前环境登记。桌面只消费投影报告，绝不自行读取这些记录或执行导入/校验流程。
- `application/coordinator` 按 runs、tasks、messages、waits、completion 划分用例，继续通过同一 Store 事务提交。`adapters/binary` 保留公共地址/哈希接口，画像、字符串及 Ghidra 生命周期在独立子模块中维护。
- `storage` 对业务修改提供 IMMEDIATE 事务，对查询提供独立只读 WAL 快照。`storage/conversations` 负责独立会话载荷及其修订号。当前状态 schema 为 5，不提供旧数据库迁移。

worker 组件划分见 `workers/runtime/README.md`。部署 Linux 运行时目录及生成的协议 schema，并安装独立 `workers/analysis` 包；入口文件不承担模块重导出。Windows 打包使用显式组件清单，不复制 Linux 运行时。

`adapters/files` 管理主机路径、目录所有权标记、链接/重解析点检查和持久化 JSON 发布，存储层直接使用这些文件能力。配置登记、盘点和清理策略仍归维护模块；帮助函数迁移不改变锁、目录认领规则或删除范围。

`bootstrap` 实现公开服务构造函数并装配具体依赖。分析用例接收 runner 工厂，start/resume 先选定配置再调用工厂；runner 和工具业务实现不回调 bootstrap。公开构造函数签名及保存配置的行为保持不变。

项目 Python 使用 `scripts/dev/requirements.txt` 和 `ruff.toml` 固定的开发专用 Ruff 版本。先运行 `python -m pip install -r scripts/dev/requirements.txt`，再运行 `python -m ruff format --check workers scripts tests evals fixtures` 和 `python -m ruff check workers scripts tests evals fixtures`。格式变更与逻辑变更分开提交。静态检查覆盖语法、控制流、无效比较及未定义名称，可选依赖的导入仍需人工审查。运行时包不安装 Ruff；生成产物、外部基准和缓存的第三方依赖不在格式化范围内。

Linux Worker 镜像通过 `pip --require-hashes --no-deps --no-build-isolation` 安装完整的 `workers/requirements.txt` 锁，其中包含运行时及传递依赖。SHA-256 哈希来自固定 AFL++ 基础镜像中为 Linux amd64/Python 3.12 选择的实际发行包；更新时必须在该环境中从下载的发行包重新计算，不能复制未经验证的值。只有完整锁有意混用元数据解析器会拒绝的 angr 9.2.x 组件版本，才需要 `--no-deps`；部分 requirements 文件绝不能使用该选项。除非相应验收证据发生变化，否则保留自包含的 `angr==9.2.78`、兼容 Python 3.12 的 `pycparser==2.21` 以及兼容 AFL `unicornafl` 的 `unicorn==2.1.4` 约束。宿主机 Linux 测试从 `scripts/ci/requirements.txt` 获取相同的 pyelftools 精确版本；镜像外的轻量运行时 requirements 仍保持独立。

跨模块调用保持类型化，资源归属和业务完成门槛保留在统一入口。通用元数据查询不读取 checkpoint；需要继承、follow-up 或完整导出时显式使用完整读取接口。元数据写入不得清空会话，checkpoint 与邮箱确认仍在同一持久化事务提交。

工作台操作在完整的归属作用域内持有 `OperationLock`。守卫会先显式解锁，再关闭文件；不要替换成析构时只关闭描述符的普通文件。在 Unix 上，无关的并发 fork 可能在 exec 前临时继承打开文件描述，导致只依赖关闭的锁超出归属作用域。描述符克隆回归检查在保持互斥的同时，后继操作能够立即获得锁。

维护准入、更新缓存访问以及清理过程中的工作台锁使用 `adapters/files/locks::FileLock`，遵循同样的显式释放规则。可能提前返回的策略检查之前就应取得守卫，并持有至整个受保护操作结束。分析任务的克隆通过 `Arc` 共享同一个守卫，由最后一个拥有者释放。共享与独占模式保留原有准入规则。分析器锁的所有权刻意延续至子进程回收完成，不能套用这种仅按作用域释放的方式。

## 本地文件与任务产物

仓库中的 `.vscode/settings.json` 在文件浏览器里隐藏依赖缓存和生成产物，不删除文件，也不改变构建输入。新的任务专用日志、清单和回执放在已忽略的 `target/tasks/<task-id>/` 下；工具管理的构建和缓存目录保持现有路径。旧证据保留其记录的路径及哈希。源码、夹具、许可证、发布片段和双语指南继续可见并纳入版本管理；清理仍须核对明确归属和保留策略。

Python 构建和发布 CLI 复用 `scripts._lib` 定位仓库、计算哈希和访问 GitHub，参数解析与可复用函数分离。构建命令受 CI 作业期限约束，夹具执行和网络下载各自有操作期限。包验证在独立信任边界重复校验哈希是有意保留的设计。共享分析包接收已准入的路径或字节，数值算法与 Worker 所有权分离；类型注解说明内部输入和输出，不替代生成的协议 schema。

测试辅助 `support.client.Client` 只拥有自己的 stdio 转发进程。它对请求写入和响应等待设置期限，核对响应 ID，并在传输结果不确定时关闭转发进程。测试显式重连并核对持久作业，或关闭守护进程；关闭转发进程不代表守护进程及其资源已清理。

<a id="desktop-styling"></a>

前端在 `ui/src/features/` 下按工作区、会话、Agent、证据、设置和维护分组。共用 API 契约、UI 基础组件、本地化及测试辅助放在 `ui/src/shared/`；`ui/src/app/` 负责装配和桌面生命周期。功能测试与实现同目录，跨功能浏览器流程放在 `ui/e2e/`。全局样式导入顺序继续集中维护。

## 桌面界面样式

界面、原生消息和报告的双语资源及验收要求见[多语言维护](localization.zh-CN.md)。用户语言偏好不得通过工作区配置保存流程修改。

`ui/src/styles.css` 只按固定顺序导入 `styles/` 中的基础、工作区、聊天、Agent、证据、设置、响应式及维护界面样式；移动规则时保持层叠顺序。
前端使用现有的 Radix、assistant-ui 和 Lucide 组件。颜色统一定义在 `ui/src/theme.css`，组件样式引用语义变量；不要在组件中重新散布颜色值。
`ui/src/workspace-layout.css` 只放分栏容器和拖动分隔条；组件的尺寸、间距、边框直接归入 `styles/` 对应文件，响应式规则统一放在 `styles/responsive.css`，不叠加一层旧布局覆盖。分栏使用 `react-resizable-panels`，与 shadcn/ui 的分栏采用同一基础组件，保留键盘调整和宽度记忆。
桌面视图偏好通过 `useViewPreference` 保存为本机 WebView 数据；任务别名按工作区路径和 run ID 区分，不能写回任务指令或作为目标身份。设置与存储草稿留在设置组件状态中，目录操作仍经过既有 epoch 与维护锁检查。

默认采用中性深灰底色，金色表示主要操作或选中，蓝色表示执行中，绿色表示成功，黄色表示等待/警告，红色表示失败。状态同时保留图标和文字。
字体栈采用桌面系统无衬线字体，代码使用系统等宽字体；统一字号变量定义在 `ui/src/theme.css`。根字号 16px，正文和输入正文 16px，导航、按钮、表单及代码 14px，时间与路径等辅助信息至少 13px；字号以 rem 表示，避免组件重新硬编码小字号。调整布局时检查桌面最小窗口下的滚动、长路径和消息草稿保留。
颜色层级参考 [Radix Colors](https://www.radix-ui.com/colors/docs/palette-composition/understanding-the-scale)，工作区分区参考 [VS Code](https://code.visualstudio.com/docs/editing/getting-started/userinterface)。

<a id="tool-documentation"></a>

## 工具文档

工具定义、参数 schema、角色权限、模型可见性和显示消息 ID 来自同一注册表；双语文案来自共享 Fluent 资源。内部入口由 `Visibility::Internal` 标记，不通过名称黑名单逐层过滤；内部事务调用保留权限校验。桌面只为原生 Rig 工具和旧事件名称保留 `interfaces/desktop/tool_aliases` 映射，新增内置工具必须在 catalog 提供消息 ID 并补齐双语标题。Markdown 工具清单通过仅供开发使用的 example 从注册表生成：

```sh
cargo run --example export_catalogs -- tool-catalog > docs/reference/tool-catalog.md
cargo run --example export_catalogs -- --locale zh-CN tool-catalog > docs/reference/tool-catalog.zh-CN.md
cargo run --example export_catalogs -- --json tool-catalog
cargo test --test tool_catalog
```

导出目录无需模型凭据，也不读取配置或状态目录。测试比较生成结果与已提交文档，防止工具改动后遗漏更新。

<a id="skill-coverage"></a>

## Skill 覆盖清单

内置方法的工具、来源及机制夹具关联记录在 `prompts/coverage.json`，复用 `SkillManifest` 的 `related_tools`、`sources`、`fixtures` 字段。更新后生成 [覆盖清单](../reference/skill-coverage.zh-CN.md)：

```sh
cargo run --example export_catalogs -- skill-catalog > docs/reference/skill-coverage.md
cargo run --example export_catalogs -- --locale zh-CN skill-catalog > docs/reference/skill-coverage.zh-CN.md
cargo run --example export_catalogs -- --json skill-catalog
cargo test --test tool_catalog
```

这些字段只用于维护和查询，不授予工具权限，也不证明当前后端可用、原题复现或 Agent 解题成功率。未登记表示没有关联记录。自定义 Skill 可在原有 manifest 中填写这些字段；覆盖内置正文时不会自动继承内置夹具声明。单元测试校验内置关联的工具、来源文件及夹具 ID 存在，导出不读取模型凭据或状态目录。

<a id="shared-validation-and-content-identity"></a>

## 共享校验与内容身份

`domain/claims` 统一解析 flag 形状的声明；完成工具拒绝占位符，协调器按具体声明执行独立验证门槛。解析结果只表示文本分类，不证明利用成功。

Rust 文件哈希和实际读取字节计数放在 `adapters/files/content`，`adapters/binary` 保留公开哈希接口；worker 使用 `wukong_worker.artifacts.common.sha256_file`，构建与发布脚本使用 `scripts._lib.checksums.sha256`。独立注入沙箱的脚本保持自包含，不引入控制端模块。路径限制、大小预算和控制端/worker 的独立身份核对仍留在调用边界，不因共用哈希函数而合并或删除。

<a id="validation"></a>

## 验证

```sh
cargo fmt --all -- --check
cargo clippy --offline --all-targets -- -D warnings
cargo test --offline --all-targets
python workers/tests/run.py unit -p test_bridge.py
```

运行 worker 测试前，在虚拟环境中安装 `workers/analysis` 和 `workers/runtime/requirements.txt` 并确保 `python` 命令可用。普通测试使用可信本地夹具和回环服务；被忽略的真实 Ghidra/Docker 验收需要按各测试说明配置环境。

同级容器网络验收以仓库根目录为构建上下文，构建 `tests/acceptance/network.Dockerfile`，先安装分析包及相同的固定版本 Linux worker 依赖，再进入隔离网络。裸 Python 镜像不包含 worker 发布元数据所需的依赖。

<a id="repeatable-performance-baseline"></a>

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

文档配对、人工语义核对和 CI 检查见[文档维护](documentation.zh-CN.md)。

<a id="readme-screenshots"></a>

## README 截图

截图使用当前前端及固定的示例任务，不运行模型或 worker。图注明确注明示例数据，不能作为分析结果或原生桌面验收。不要使用可能过时的手工界面拼图。

安装根目录 npm 依赖及 Playwright Chromium 后，在一个终端运行 `npm run dev`，保持本地服务位于 `http://127.0.0.1:1420`；另一个终端在仓库根目录执行：

```sh
node scripts/dev/capture-readme.mjs
```

脚本复用前端夹具结构，提供限定的原生命令适配器，生成 `docs/assets/workspace.en-US.png` 与 `docs/assets/workspace.zh-CN.png`。固定视口、时区和示例数据，不读取本机配置或凭据；失败、未处理错误或横向溢出会中止生成。逐张检查裁切、字体、工具行和协作树，再与对应 README 一起提交。首次缺少浏览器时运行 `npx --no-install playwright install chromium`。

要检查真实 CLI 执行到桌面的投影，可将 `WUKONG_CAPTURE_WORKSPACE` 设为 `target/runtime-workspace.json`，运行 `cargo test --locked --test cli_workflow cli_runs_dynamic_agents_real_rig_and_exports_evidence`。测试复用本地脚本化 provider、真实 Rig 调度器、SQLite 和静态二进制检查，不调用付费模型，也不执行夹具。Windows 需使用 MSVC 开发者终端及隔离的测试 `LOCALAPPDATA` 目录。前端服务运行后，可渲染捕获的数据：

```sh
node scripts/dev/capture-readme.mjs target/runtime-workspace.json
```

这些诊断图片写入 `target/runtime-workspace/`，与 README 示例图片分开。适配器仅提供桌面传输接口；Agent、文本、工具结果、目标和证据 ID 均来自捕获的后端响应。此验收证明 CLI 到桌面的展示链路，不证明真实模型的推理质量，也不覆盖真实 Ghidra 或调试器执行。原生流式验收还会保持 HTTP 响应未结束，直到能从桌面快照读取公开的部分回复，并检查最终消息去重以及内部推理不进入展示。

恢复测试还会暂停一个尚未结束的响应，恢复原生执行日志，并确认最终文本更新的是同一个公开消息 ID。
