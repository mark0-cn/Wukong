# 维护与性能检查

## 模块边界

- `kernel` 只装配 Rig 应用和驱动生命周期。`admission` 管理准入，`models` 管理模型请求与计量，`coordination` 管理消息/状态投影，`business_tools` 接入工具效果。`kernel_host` 暴露业务操作，`supervisor` 独立管理心跳和后台任务观察；`model_catalog`、`usage`、`bootstrap` 分别负责模型选择、用量和装配。
- `tools` 保留执行管线和共同校验入口，`effects` 管理持久化回执；`catalog` 统一注册 schema、角色、恢复策略与处理器，`handlers` 执行业务路由，`coordination` 处理共享资料与任务，`analysis` 处理静态查询，`execution` 分派 worker 请求，`files`、`jobs`、`sessions` 各自管理资源。
- `queries` 定义共享查询与 DTO，`store/workspace`、`store/report_queries` 实现 SQL；`gui_data` 负责后台查询结果与展示数据适配；GUI 绘制只消费缓存。`gui_chat` 负责增量解析和可见聊天块布局。
- `store` 对业务修改提供 IMMEDIATE 事务，对查询提供独立只读 WAL 快照。`store/conversations` 负责独立会话载荷及其修订号。当前状态 schema 为 3，不提供旧数据库迁移。

worker 组件划分见 `workers/runtime/README.md`。部署整个运行时目录，入口文件不承担模块重导出。

跨模块调用保持类型化，资源归属和业务完成门槛保留在统一入口。通用元数据查询不读取 checkpoint；需要继承、follow-up 或完整导出时显式使用完整读取接口。元数据写入不得清空会话，checkpoint 与邮箱确认仍在同一持久化事务提交。

## 工具文档

工具定义、参数 schema 和 Markdown 工具清单来自同一注册表：

```sh
cargo run -- tool-catalog > docs/reference/tool-catalog.md
cargo run -- --json tool-catalog
cargo test --test tool_catalog
```

导出目录无需模型凭据，也不会打开配置或状态目录。测试比较生成结果与已提交文档，防止工具改动后遗漏更新。

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

`long_conversations_only_layout_visible_blocks` 检查 2000 条聊天消息仅排版可见块。增量解析测试验证工具结果只更新其对应聊天块，压缩后重建索引；存储测试验证有写事务时仍可读取已提交快照，元数据更新与重复 checkpoint 不改写会话。

评估后续改动时记录数据规模、构建模式和硬件环境，并分别观察界面帧耗时、查询等待、内存和数据库写入量。审计记录、证据与恢复边界的持久化不可因 UI 降频而丢失。
