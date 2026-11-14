<a id="implementation-contract"></a>

# 实现契约

[English](implementation-contract.md) · 简体中文

[历史架构方案](../history/strix-rig-binary-agent-plan.zh-CN.md)是需求基线。本文记录 Rust 实现契约及验收所需证据。

<a id="module-ownership"></a>

## 模块归属

`domain` 定义持久化记录；`store` 负责 SQLite 事务与内容寻址产物；`coordinator` 实现业务任务、契约与邮箱语义；`prompts` 注册内置元数据/资源，`agents` 装配选定角色与 Skill；`llm` 适配服务连接和类型化摘要；`runner` 管理运行生命周期，通过监督器与业务服务处理租约、用量及事件；`kernel` 将原生 Rig ECS、日志和子任务协议接入业务准入/完成门槛；`tools` 使用 Rig schema 分派类型化操作；`binary`、`runtime` 实现外部后端；`mcp` 使用 rmcp；`report` 导出有证据的发现；`interface` 提供 CLI；`desktop`、`ui/src` 提供原生桌面工作区。

<a id="required-invariants"></a>

## 必须保持的不变量

1. 创建子任务先持久化并返回，不等待子任务完成。依赖默认 all_success；all_terminal、any_success 必须显式指定。无法满足的前提在模型执行前阻止后继并通知父任务。业务等待与创建依赖共享循环检查；原生请求/等待生命周期由 Rig 管理。父子身份、scope、skills 在重启后保留。
2. 消息持久化；included 回执仅表示进入原生会话/上下文，不表示模型执行或处理。终态通知按激活代次事务去重并绑定原生报告。活动 run 内，completed 子任务可接收显式 follow_up_agent，保留历史/累计用量；generation 区分新效果与旧回执。消息不能重开 stopped/failed 分支或终态分析。information 不唤醒等待、不重开任务。included 不等于关联回复；旧代次回复保留审计记录而不注入当前任务。
3. root 只编排，不调用目标分析/执行工具；仅在活动后代结束或被明确处理后完成。
4. 同一分析共享笔记、待办、发现和覆盖；安全边界任务可选 threat model。每个 Agent 有独立检查点/会话。压缩覆盖全部共享文档类型并保留原始检查点证据；协调消息进入可压缩历史，不进入无限增长的系统前缀。
5. Agent/工具状态变更持久化。已完成的副作用不盲目重做；不明确的在途效果先与 worker 对账。
6. 调试修改按 owner 串行。Agent 完成前关闭自己的调试/交互会话，root 检查所有会话。同 run 其他 Agent 可关闭终态 owner 的孤儿会话，但不能发命令。长 fuzz 使用独立于模型状态可对账的 job handle。
7. 地址携带目标/模块身份及地址空间；缓存与证据绑定目标、工具、配置身份。
8. 提升发现影响需要独立 verifier 记录及原始证据。静态怀疑或崩溃本身不证明利用原语。
9. 模型上下文中的结果有界，完整哈希产物可追溯读取。
10. 模型没有配置后端之外的执行通道；模型凭据只在控制端。
11. 契约声明输入、约束、产物及完成条件。成功结果覆盖所需输出并关联原始证据；非成功解释缺口。每代结果归档。结构/来源校验不是事实的独立验证。当前面向新状态目录/任务，不迁移旧 schema。
12. 原生创建在前驱完成前返回；业务依赖门槛阻止模型 I/O，并在放行时提供前驱结果。Rig 负责通用效果、重试、会话配对及子任务报告。工具对账使用稳定 wire call ID，而非恢复后的 ECS ID。继承只包含闭合的父对话。
13. 模型 I/O 前保存解析后的 endpoint/配置；恢复不受环境默认地址改变影响。角色输出预算与模型容量分离。缺少用量/费用信息时保存响应，并在工具 I/O 前暂停。MCP 只读提示不能授权重放不确定效果。
14. 角色只有 root、analyst、verifier。能力 Skill 补充方法/产物，不改变角色、权限、完成协议。角色指令不能作为 Skill 加载。同范围活动 analyst 跨能力共享去重 owner；独立验证使用另一 verifier 实例。普通报告归 root。

<a id="acceptance-evidence"></a>

## 验收证据

单元/集成测试覆盖事务生命周期、并发消息、去重、检查点重放、发现门槛、工具路径约束与真实二进制解析。端到端运行实际 CLI。已安装外部工具时验证真实适配器；区分适配器和环境依赖测试，不能把 mock 协议称为完整 Ghidra/GDB/AFL 验收。

<a id="delivery-audit"></a>

## 交付审计

跟踪 CLI/配置、动态调度、持久化/恢复、静态分析、Ghidra、GDB/MI、AFL++、崩溃缩减、独立验证、Skills、MCP、GUI、JSON/Markdown/HTML 报告、重放产物、Docker、评测夹具及回归。编译通过不足以宣称项目完成。

<a id="structured-method-deliverables"></a>

## 结构化方法产物

规范产物名 `formats`、`algorithms`、`state_machine`、`binary_diff` 要求匹配 `structured.kind`，包含 1–256 行；每行引用同 run 的 `evidence_id`。必需字段分别为 formats：offset/width/encoding/meaning/confidence；algorithms：input/expected/observed/arithmetic；state_machine：from/action/to/expected/observed；binary_diff：baseline_sha256/target_sha256/before/after。摘要为 64 位十六进制。另行关联原始产物；结构完整不等于正确，独立 verifier 必须挑战语义声明。旧的非规范产物名继续支持。

普通交付物只需契约中的精确名称和原始 `evidence_id`；不要求方法行时省略 `structured`。完成条件使用契约中的精确标签。校验会指出缺失的交付物名称，并一次列出首个无效行的全部缺失字段，避免每轮猜测一个字段来修正结果。
