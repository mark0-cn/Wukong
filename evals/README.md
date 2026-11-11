<a id="行为评测与独立评分"></a>

# Behavior evaluation and independent scoring

English · [简体中文](README.zh-CN.md)

Evaluators/scorers live here; sample source/tasks/human ground truth remain in [fixtures](../fixtures/README.md). Invoke Python modules from the repository root. Default outputs remain under corresponding `fixtures/*/build`, or choose independent directories.

```sh
python -m evals.analysis.evaluate --build-dir /tmp/wukong-analysis
python -m unittest discover -s evals/tests -p 'test_*.py'
sh fixtures/build.sh /tmp/wukong-vulnerability
python -m evals.vulnerability.evaluate --build-dir /tmp/wukong-vulnerability --repetitions 3
```

Vulnerability evaluation requires Linux x86_64; analysis fixtures need C11. Tests execute only trusted project samples. Behavior success is not Agent discovery/answer quality; report scoring requires independent review bound to original evidence/target hashes. Do not expose source, truth or scoring information to the assessed Agent. See [fixture input formats](../fixtures/README.md) and [paired experiments](analysis/experiments.md).
