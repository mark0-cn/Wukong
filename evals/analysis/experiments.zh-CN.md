<a id="agent--skill-paired-evaluation"></a>

# Agent / Skill 配对评测

[English](experiments.md) · 简体中文

在两种条件下运行已有分析夹具，每次只改变一个变量：

- Skill 消融：选定方法指令与该方法的空自定义覆盖比较，保留角色与权限。
- 协作：一个 analyst 与多个限定范围的 analyst，模型和总预算一致。
- 验证：向独立 verifier 提交故意错误的声明（长度包含头、字节序错误、溢出宽度错误、reset 保留状态）。预期判定留在复核资料中，不进入提示词。
- 能力缺失：关闭 Ghidra 或执行能力后重复，独立检查回答是否限定范围并指出缺失能力。

每例至少重复三次。导出真实 report.json，独立判定每道问题，包括 unresolved。比较程序不调用模型或伪造判定。模型、预算及目标哈希来自运行配置，不凭记忆。声明的实验条件只是待核对假设：检查实际 Agent/Skill 轨迹确认执行。

`python -m evals.analysis.compare_runs manifest.json --output comparison.json`

manifest 是 JSON 数组，每行含 `case`、`repeat`、`condition`（baseline/candidate）、`model`、`budget`、`target_sha256`、`questions`（ID）、`report`（相对路径）、`reviews`（相对路径）。复核文件为数组，含 question_id、verdict（correct/incorrect/unresolved）、rationale、evidence_ids。错误声明复核还含 injected_false_claim=true 和 accepted_by_agent=true/false。拒绝缺失复核、未配对运行及模型/预算/哈希/问题集合不一致。

指标包括证据支撑正确率、错误/未解决率、Token、配置费用、可用时的耗时、失败调用、重复读取代理指标、Agent 数量、不可变通用验证记录和错误声明接受率。先计算每对 candidate-minus-baseline，再取平均；不能自动证明 Skill 改善质量。
