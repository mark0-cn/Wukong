# Worker 协议 v3

[English](README.md) · 简体中文

验证期望只匹配已设置的条件。省略或为 null 的可选字段不增加约束，Rust 序列化时会省略这些字段。如果没有剩余条件，验证仍须观测到崩溃，不能将正常退出视为成功。

`job.list` 和 `resource.list` 接受可选的排他性 `after` 游标，以及 1–64 的 `limit`（默认 64）。作业 ID 按升序返回；使用最后一项 ID 继续查询，直到出现空页。资源分页还提供 `next_after`。Linux 作业清单遵循与原生 Windows 相同的游标语义和范围限制。

`debug.start` 可通过已暂存的 `target` 或 `launch_spec` 选择目标；控制器会核对所有提供的启动描述及输入产物均属于当前任务。原生 Windows 的 LaunchSpec 负责目标和参数，因此此模式下应省略单次调用的 `target` 和 `args`。调试器默认模式为 `launch`，默认引擎为 `auto`。Linux 在解析执行环境前，从校验后的 LaunchSpec 中确定原始目标哈希。

`exec.run`、`crash.replay`、`input.minimize` 和 `poc.verify` 接受可选的 `output_limit`，范围为 1,024–8,388,608 字节（默认 1 MiB），与两种平台的执行器一致。该上限限制保留的执行输出（重放／最小化按每次试验计算），与控制器面向模型的输出预览限制不同。生产者／验证者身份仅属于 `poc.verify` 请求。

本 crate 负责公共方法标识、请求类型、资源观察、请求上下文、错误和容量契约，不依赖控制器或平台 Worker。Windows 的 JSON-RPC 信封由 jsonrpsee 处理，Linux 则使用 jsonrpcserver。SSH 和 stdio 仍是传输方式；原生内部会话使用 Protobuf。

`src/methods.rs` 声明每个方法的输入／输出类型、副作用策略和资源需求。平台注册目录将这些方法绑定到实际处理器及其就绪条件。模型工具目录直接声明所需方法，不再通过工具名称推断。

执行 `cargo run --manifest-path workers/protocol/Cargo.toml --bin export` 生成共用 JSON Schema；追加 `-- --check` 可检查仓库中的生成文件。Linux 使用 jsonschema 校验请求和结果。Rust 与 Python 读取同一组 `fixtures/contract.json` 样本，覆盖非法输入、结果、错误、身份和能力声明。

拒绝接入协议 v2 Worker。Linux 状态记录固定为协议 v3，并拒绝旧版安装记录；请使用新的 Worker 状态目录。保留旧目录和证据，不自动迁移或清理。

副作用回执与控制器任务收据分开。已完成请求复用原结果；标记为 `once` 的中断操作返回 `outcome_unknown`，不能自动重放。只有注册为 `reconcile` 的操作可以核对各自的持久化状态。独立控制请求不会等待无关操作的执行回执。
