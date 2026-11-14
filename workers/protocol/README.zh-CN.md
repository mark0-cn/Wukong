# Worker 协议 v3

[English](README.md) · 简体中文

本 crate 负责公共方法标识、请求类型、资源观察、请求上下文、错误和容量契约，不依赖控制器或平台 Worker。Windows 的 JSON-RPC 信封由 jsonrpsee 处理，Linux 则使用 jsonrpcserver。SSH 和 stdio 仍是传输方式；原生内部会话使用 Protobuf。

`src/methods.rs` 声明每个方法的输入／输出类型、副作用策略和资源需求。平台注册目录将这些方法绑定到实际处理器及其就绪条件。模型工具目录直接声明所需方法，不再通过工具名称推断。

执行 `cargo run --manifest-path workers/protocol/Cargo.toml --bin export` 生成共用 JSON Schema；追加 `-- --check` 可检查仓库中的生成文件。Linux 使用 jsonschema 校验请求和结果。Rust 与 Python 读取同一组 `fixtures/contract.json` 样本，覆盖非法输入、结果、错误、身份和能力声明。

拒绝接入协议 v2 Worker。Linux 状态记录固定为协议 v3，并拒绝旧版安装记录；请使用新的 Worker 状态目录。保留旧目录和证据，不自动迁移或清理。

副作用回执与控制器任务收据分开。已完成请求复用原结果；标记为 `once` 的中断操作返回 `outcome_unknown`，不能自动重放。只有注册为 `reconcile` 的操作可以核对各自的持久化状态。独立控制请求不会等待无关操作的执行回执。
