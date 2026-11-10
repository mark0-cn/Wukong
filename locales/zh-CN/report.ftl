report-title = 二进制分析报告
report-run = 任务
report-target = 目标 SHA256
report-status = 状态
report-task = 任务要求
report-summary = 总结
report-no-summary = 尚未记录最终总结。
report-findings = 发现
report-no-findings = 尚未记录发现；这不证明目标不存在漏洞。
report-stage = 记录的验证阶段
report-unconfirmed = 影响已验证（记录中的声明；独立验证缺失或不完整）
report-root-cause = 根因
report-impact = 报告的影响
report-severity = 严重程度
report-severity-rationale = 评级依据
report-replay = 独立重放
report-attempts = 次成功尝试
report-verified-impact = 已验证的影响
report-evidence = 证据标识
report-coverage = 覆盖范围
report-scope = 范围
report-kind = 类型
report-outcome = 结果
report-notes = 备注
report-accounting = 用量
report-accounting-body = { $agents } 个 Agent；{ $calls } 次工具调用；{ $evidence } 条证据；{ $tokens } tokens；费用 { $cost } 美元。
report-raw-guide = 环境、条件、完整工具参数和证据引用见 report.json 与 replay.json。
report-stage-suspected = 待验证
report-stage-reproduced = 已复现
report-stage-root_cause_confirmed = 根因已确认
report-stage-impact_verified = 影响已验证
report-stage-inconclusive = 尚无定论
report-stage-rejected = 已排除
report-status-queued = 等待执行
report-status-running = 进行中
report-status-waiting = 等待协作
report-status-budget_paused = 已暂停
report-status-completed = 已完成
report-status-stopped = 已停止
report-status-crashed = 异常退出
report-status-failed = 失败
report-bundle-readme = Wukong 证据包

    report.md / report.html：发现与验证状态的可读报告。
    report.json：事务一致的任务快照及原始证据索引。
    replay.json：精确记录的工具调用、验证环境和条件。
    config.redacted.json：凭据字段已脱敏的配置。
    target/sample.bin：原始目标字节（导出时不执行）。
    evidence/：按 SHA256 索引的原始证据字节。
    manifest.json / SHA256SUMS：全部文件的摘要。使用 wukong verify-bundle 校验。

    校验和证明完整性，不证明发布者身份。原始证据不脱敏。
    脱敏配置和事件元数据不能用于备份凭据。
