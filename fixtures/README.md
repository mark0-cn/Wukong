# 本地回归与评测集

## 通用逆向分析样本

`analysis/` 补充格式解析、对象布局、32 位变换、交互状态机和版本差异任务。
`analysis/tasks.json` 定义用户问题与逐题评分项，不以漏洞发现或 flag 为完成条件。

```sh
python3 -m evals.analysis.evaluate --build-dir /tmp/wukong-analysis-fixtures
python3 -m evals.tests.test_analysis
```

行为 harness 使用 C11 编译器构建四个可信程序，并检查有效/无效记录、整数边界和
命令序列。`CC` 可指定编译器；这些检查可在本机运行，但要供当前动态分析后端使用，
须在 Linux x86_64 环境构建目标。生成文件记录源码/目标哈希、编译命令与实际输出。
`fixture_checks_passed` 只代表样本行为回归通过，`agent_accuracy` 保持 null。

运行 agent 时只提供目标二进制和对应 task 文本；盲测不提供 C 源码、`results.json`
或构建元数据。版本差异任务额外提供生成的 `baseline_profile.json` 内容作为分析上下文，
它包含基线版本的原始输入与实际观察；缺少基线时应报告缺口。

实际回答质量由独立复核逐题判定。报告映射仍为 case ID 到导出的 `report.json` 路径；
复核文件示例（每个已运行任务的每道问题都须有标签）：

```json
{"labels":[{"case":"record-format","question_id":"field_layout","verdict":"correct","evidence_ids":["ev_actual_id"],"rationale":"独立对照实际 parser 与样本，字段边界和字节序一致"}]}
```

```sh
python3 -m evals.analysis.score --build-dir /tmp/wukong-analysis-fixtures \
  --reports reports/analysis-index.json --adjudications reports/analysis-reviewed.json \
  --output reports/analysis-metrics.json
```

评分器读取当前真实导出格式的 `snapshot`，核对样本哈希，要求正确标签关联该报告中的
证据 ID；不检查标题关键词，也不要求创建漏洞 finding。指标包括逐题正确/错误/未解决数、
有证据支撑的正确率、缺失任务和模型用量。未运行任务保持未解决；这些小样本不足以支持
对真实软件逆向准确率的统计结论。

## 漏洞与崩溃回归基线

这套样本用于验证项目能力：有符号与 stripped ELF、一个有明确源码根因的栈越界样本、对应修复版本，以及正常程序。样本只接受本地文件输入；不包含网络目标或现成利用载荷。

在 Linux x86_64 环境中执行：

```sh
sh fixtures/build.sh
python3 -m evals.vulnerability.evaluate --repetitions 3
```

`CC`、`STRIP` 可指定编译器工具。构建需要 C 编译器、binutils 与 Python 3；产物写入 `fixtures/build/`。漏洞版本关闭栈保护与 PIE，使越界回归不依赖具体保护器诊断，保留 NX；正常程序启用 PIE、栈保护与完整 RELRO。`overflow.bin` 是 256 个 `A`，只用于观察本地崩溃。

`manifest.json` 是人工从源码确定的 ground truth，不能作为模型输入。`evaluate.py` 实际重复执行六个程序，用退出码/信号验证正常路径、漏洞路径和修复路径，限制时间、内存并禁止 core dump。`results.json` 记录样本/输入/源码哈希、原始结果与耗时。33 个行为检查通过，**不代表 agent 自动发现了漏洞**；检测指标保持 `null`。

实际 agent 检测评测应在每个二进制上单独建 run，将 JSON 报告保存到独立路径，然后建立报告映射，例如：

```json
{"overflow-vuln":"reports/vuln.json","overflow-fixed":"reports/fixed.json"}
```

独立复核每个被声明为 reproduced、root_cause_confirmed 或 impact_verified 的发现后，填写判定文件；`truth_id:null` 表示误报：

```json
{"labels":[{"case":"overflow-vuln","finding_id":"finding_actual_id","truth_id":"copy-overflow","rationale":"已独立核对输入、memcpy 长度与栈缓冲区大小及对应原始证据"}]}
```

```sh
python3 -m evals.vulnerability.score_reports --reports reports/index.json \
  --adjudications reports/reviewed.json --output reports/metrics.json
```

评分前检查目标 SHA256；没有独立标签的确认发现会报错，不会按标题、CWE 或崩溃现象自动判为正确。重复报告同一个根因计为重复误报；缺失 run 的已知正例计为漏报。指标包括精确率、召回率、独立验证成功率、token 数和配置价格估算。源码根因、利用原语与影响范围仍需复核，尤其不能用一次崩溃推导代码执行。

这个小型评测集属于回归基线；没有足够样本支持对真实软件漏洞发现率的统计结论。后续可以按输入格式、保护机制、编译器和优化等级扩展 manifest，但每个正例必须同时保留修复版本与可验证根因。
