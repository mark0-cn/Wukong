# 行为评测与独立评分

评测程序与评分器位于本目录；样本源码、任务和人工真值保留在 [fixtures](../fixtures/README.md)。
从仓库根目录用 Python 模块方式运行。默认构建输出仍位于对应的 `fixtures/*/build`，也可指定独立目录。

```sh
python -m evals.analysis.evaluate --build-dir /tmp/wukong-analysis
python -m unittest discover -s evals/tests -p 'test_*.py'
sh fixtures/build.sh /tmp/wukong-vulnerability
python -m evals.vulnerability.evaluate --build-dir /tmp/wukong-vulnerability --repetitions 3
```

漏洞样本行为评测需要 Linux x86_64；分析样本需要 C11 编译器。测试只执行项目的可信样本。
行为通过不代表 Agent 发现能力或回答质量通过；报告评分仍需独立复核，且与原始证据和目标哈希关联。
源码、真值和评分信息不提供给被测 Agent。评分输入格式见 [fixtures 说明](../fixtures/README.md)，配对实验见[实验说明](analysis/experiments.md)。
