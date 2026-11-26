<a id="evidence-report-and-replay-bundle"></a>

# 证据报告与重放包

[English](report-format.md) · 简体中文

CLI `analyze`（含 `--resume`）先导出指定的报告包并输出已保存的结果，再检查是否完成。只有 `completed` 退出码为 0；暂停、停止、崩溃、失败及其他未完成状态均返回 1，并在 stderr 标明状态。通过结构化结果的 `status` 区分可恢复的预算暂停与失败。导出成功和报告包完整性不代表分析完成或语义正确。

`wukong::report::export(store, run_id, output)` 导出一个事务快照。读取并哈希原始目标和每份证据，不执行样本。暂存目录内全部载荷验证通过后才重命名为目标目录。目标不存在时会新建目录；已有目录只有在属于同一 run 且通过完整 bundle 校验时才可刷新。替换失败会恢复旧 bundle；损坏的包、符号链接和其他 run ID 均被拒绝。

`export_with_locale(store, run_id, output, locale)` 翻译 Markdown/HTML 标题、记录状态和包内 README。兼容入口 `export` 默认英文；桌面采用当前界面语言，CLI 使用 `--locale`。已有正文、快照字段、重放参数、标识、哈希及证据字节不变；只有展示文件的内容哈希变化，完整性校验仍覆盖全部文件。

HTML 使用 Askama 模板及自动转义；Markdown 单独转义标点。二者均不改写原始证据。

包内文件：

| 文件 | 内容 |
| --- | --- |
| `report.md`, `report.html` | 发现、记录阶段、独立验证状态、覆盖及用量 |
| `report.json` | run、Agent、检查点、发现、验证、覆盖、文档、调用、job、消息、事件 |
| `replay.json` | 精确工具参数、目标/证据映射、验证环境和条件 |
| `config.redacted.json` | 凭据字段替换为 `[REDACTED]` 的运行配置，保留环境变量名 |
| `target/sample.bin` | 原始目标字节，保留 run 的 SHA256 |
| `evidence/XX/SHA256` | 去重后的原始证据字节 |
| `manifest.json`, `SHA256SUMS` | 全部载荷及 manifest 的大小和 SHA256 |

`verify_bundle(output)` 检查摘要、大小、目标映射、校验索引、相对路径边界、符号链接逃逸及意外文件。校验和检测变更，不认证发布者。原始证据可能含敏感程序数据；元数据/配置脱敏不会改写它。

发现保留记录阶段和基于证据的严重性（critical/high/medium/low/info）及理由。critical/high 仅接受 reproduced 或更高阶段；单纯崩溃最高 medium。`impact_verified` 若没有成功验证记录、非零有效尝试次数、证据和另一位角色为 verifier 的 Agent，会明确标为不完整。导出不会从崩溃推断代码执行、提升阶段或编造缺失的重放条件。

当前覆盖范围不包含结果为 `superseded` 的行。Markdown 和 HTML 将这些行保留在独立的审计历史部分，`report.json` 则保留所有行及每个 root 解决记录的 `supersedes` 关联。历史备注和证据不是当前结论或未解决工作。新证据反驳某个推断时，当前记录必须更正该推断，不能把被取代的文本当作仍然成立的事实。

动态 trace job 将 `trace/run.json`、`trace/events.jsonl` 和私有调试器 transcript 保存为原始证据。`trace_compare` 另行生成 `trace.comparison` 产物，关联两侧源 job/证据、逻辑 plan 和 comparison signature。事件流完成、观察完整性、job settled 和证据就绪仍是不同事实；部分 trace 可以导出用于诊断，但不能据此得到 `match`。当前后端和限制见[动态函数追踪与离线差分](../user/dynamic-trace.zh-CN.md)。

重放前先验证包，按调用/验证记录重建运行环境及依赖，将原样本路径映射到 `target/sample.bin`，从 `bundle_path` 导入证据，再执行记录参数。凭据通过原环境变量名提供。包不内置运行时快照或第三方引擎安装，其身份和要求保留在元数据中。

通用分析 `analysis_verify` 记录必须提供 `checks`（1–64 项）和 `limitations`。每项检查绑定声明、期望/观察值、判定及验证者自己的证据；整体 `supported` 判定不能包含被反驳或不确定的检查。复现检查还必须绑定精确产物证据、验证者针对同一目标执行的作业、原始状态证据及声明的预期退出码；保存的参数保留实际调用。覆盖范围明确仅限所列检查，不代表整份报告。应分开验证输入接受性、推导过程、边界行为和命令复现。这些来源门禁不能证明观察声明在语义上正确。

## 脚本判定验收补充

`analysis_verify` 自动解析自有任务的原始状态证据，并将请求参数与解析后的绑定分别保存。诊断按索引列出所有无效检查。进程正常结束且退出码符合明确声明的非零预期时，可支持负向复现；信号、超时、取消和丢失任务不能冒充正常拒绝。

`report.json.snapshot.artifact_validations` 和 `replay.json.artifact_validations` 保留 `self_check_v1` 验收记录，绑定最终脚本与基线哈希、目标、verifier、worker、基线/驱动任务、原始状态及完整报告证据。任务状态提供有界验收摘要和失败场景；完整用例保留为原始证据。修订后需新记录。未声明该配置的历史脚本不视为通过此验收。

[validation 方法](../../prompts/support/validation.md) 定义 `--validation-io`（stdin 一条 JSON 请求、stdout 一条 JSON 响应）、正常 `--verify` 基线采集及共用判定器。支持 Python 和 POSIX Shell；Shell 示例明确依赖 Python 3。驱动在配置的 worker 内测试模拟观测。通过不证明采集路径连接正确、所有声明均被覆盖、目标行为正确或真实发生过注入故障；这些仍需独立审查和真实目标证据。
