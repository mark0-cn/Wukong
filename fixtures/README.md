# 本地回归与评测集

这套样本用于验证项目能力：有符号与 stripped ELF、一个有明确源码根因的栈越界样本、对应修复版本，以及正常程序。样本只接受本地文件输入；不包含网络目标或现成利用载荷。

在 Linux x86_64 环境中执行：

```sh
sh fixtures/build.sh
python3 fixtures/evaluate.py --repetitions 3
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
python3 fixtures/score_reports.py --reports reports/index.json \
  --adjudications reports/reviewed.json --output reports/metrics.json
```

评分前检查目标 SHA256；没有独立标签的确认发现会报错，不会按标题、CWE 或崩溃现象自动判为正确。重复报告同一个根因计为重复误报；缺失 run 的已知正例计为漏报。指标包括精确率、召回率、独立验证成功率、token 数和配置价格估算。源码根因、利用原语与影响范围仍需复核，尤其不能用一次崩溃推导代码执行。

这个小型评测集属于回归基线；没有足够样本支持对真实软件漏洞发现率的统计结论。后续可以按输入格式、保护机制、编译器和优化等级扩展 manifest，但每个正例必须同时保留修复版本与可验证根因。
