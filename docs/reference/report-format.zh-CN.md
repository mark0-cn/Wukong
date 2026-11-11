<a id="evidence-report-and-replay-bundle"></a>

# 证据报告与重放包

[English](report-format.md) · 简体中文

`wukong::report::export(store, run_id, output)` 将一个事务快照导出到**新目录**。读取并哈希原始目标和每份证据，不执行样本。暂存目录内全部载荷验证通过后才重命名为目标目录；拒绝已有目标。

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

重放前先验证包，按调用/验证记录重建运行环境及依赖，将原样本路径映射到 `target/sample.bin`，从 `bundle_path` 导入证据，再执行记录参数。凭据通过原环境变量名提供。包不内置运行时快照或第三方引擎安装，其身份和要求保留在元数据中。
