<a id="本地回归与评测集"></a>

# Local regressions and evaluation fixtures

English · [简体中文](README.zh-CN.md)

[Pwn method regressions](pwn/README.md) add trusted fixtures/negative controls for FILE, exit, filters, heap lifecycle, input conversion, compression, VM handles and musl. Behavior checks remain separate from Agent scoring.

<a id="通用逆向分析样本"></a>

## General reverse-engineering samples

`analysis/` covers formats, object layout, 32-bit transformations, interactive state machines and version differences. `analysis/tasks.json` defines user questions/per-question scoring, not vulnerability/flag completion requirements.

```sh
python3 -m evals.analysis.evaluate --build-dir /tmp/wukong-analysis-fixtures
python3 -m evals.tests.test_analysis
```

The C11 harness builds four trusted programs and checks valid/invalid records, integer boundaries and command sequences. `CC` selects the compiler. Local checks are supported, but current dynamic-backend targets must be built on Linux x86_64. Records include source/target hashes, compiler commands and actual output. `fixture_checks_passed` means behavior regression only; `agent_accuracy` remains null.

Give the Agent only the target binary/task text, not C source, `results.json` or build metadata. For version differences also provide generated `baseline_profile.json`, containing baseline raw inputs/observations; missing baselines must be reported.

Independent review judges each answer. Map case IDs to exported `report.json` paths. Every question of each executed task needs a label, for example:

```json
{"labels":[{"case":"record-format","question_id":"field_layout","verdict":"correct","evidence_ids":["ev_actual_id"],"rationale":"Independently compared the parser and samples: field boundaries and byte order match"}]}
```

```sh
python3 -m evals.analysis.score --build-dir /tmp/wukong-analysis-fixtures \
  --reports reports/analysis-index.json --adjudications reports/analysis-reviewed.json \
  --output reports/analysis-metrics.json
```

Scoring reads actual export `snapshot`, verifies sample hashes and requires correct labels to reference evidence in that report. It does not match title keywords or require vulnerability findings. Metrics cover correct/incorrect/unresolved questions, evidence-supported accuracy, missing tasks and model usage. Unrun tasks remain unresolved; these small samples do not support statistical claims about real-software reverse engineering.

<a id="漏洞与崩溃回归基线"></a>

## Vulnerability and crash baseline

Fixtures include symbol/stripped ELF, a stack overflow with a source-confirmed cause, its fix and a normal program. They take local file input only, with no network targets or ready-made exploit payloads.

Run on Linux x86_64:

```sh
sh fixtures/build.sh
python3 -m evals.vulnerability.evaluate --repetitions 3
```

`CC`/`STRIP` select tools. Builds need a C compiler, binutils and Python 3; output goes to `fixtures/build/`. Vulnerable builds disable stack protection/PIE so regression does not depend on a specific protector diagnostic, while retaining NX. Normal builds enable PIE, stack protection and full RELRO. `overflow.bin` is 256 `A` bytes for local crash observation only.

`manifest.json` is human source-derived ground truth, never model input. `evaluate.py` repeatedly runs six programs, checking normal/vulnerable/fixed paths via exits/signals with time/memory limits and no core dumps. `results.json` stores sample/input/source hashes, raw results and durations. Passing 33 behavior checks **does not mean the Agent discovered a vulnerability**; detection metrics remain `null`.

Create a separate Agent run for each binary, save JSON reports separately and map them:

```json
{"overflow-vuln":"reports/vuln.json","overflow-fixed":"reports/fixed.json"}
```

Independently review every reproduced/root_cause_confirmed/impact_verified finding before labeling; `truth_id:null` denotes a false positive:

```json
{"labels":[{"case":"overflow-vuln","finding_id":"finding_actual_id","truth_id":"copy-overflow","rationale":"Independently checked input, memcpy length, stack buffer size and linked original evidence"}]}
```

```sh
python3 -m evals.vulnerability.score_reports --reports reports/index.json \
  --adjudications reports/reviewed.json --output reports/metrics.json
```

Scoring verifies target SHA256 first. Confirmed findings without independent labels fail instead of being accepted by title/CWE/crash. Duplicate root causes count as duplicate false positives; missing runs for known positives count as misses. Metrics include precision, recall, independent verification success, tokens and configured-price estimates. Source cause, primitive and impact still need review; one crash cannot imply code execution.

This is a small regression baseline, not statistically sufficient evidence of real-software vulnerability discovery rates. Extend by format, mitigation, compiler/optimization, but retain a fixed counterpart and verifiable cause for every positive.
