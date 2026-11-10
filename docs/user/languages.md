# 语言设置

[English](../en/user/languages.md)

在“工作区设置”顶部可以分别选择：

| 设置 | 选项 | 生效范围 |
| --- | --- | --- |
| 界面语言 | 跟随系统、简体中文、English | 立即更新菜单、状态、工具标题、对话框和日期/数字显示 |
| 新任务输出语言 | 跟随任务语言、简体中文、English | 新创建任务的 Agent 进展说明、发现和结论 |

界面语言可在分析运行中切换。消息草稿、配置草稿、选中的任务和已有结果保留。未支持的系统语言回退英文。安装时选择的 NSIS 语言不改变应用的语言偏好。

“跟随任务语言”按初始描述选择默认值：含汉字选中文，含 ASCII 字母选英文，否则使用界面语言。混合语言建议显式选择。任务中明确要求的输出语言优先；已有任务和恢复任务不会因为修改设置而更换保存的默认语言。

工具标识、命令、原始日志、文件内容和证据不翻译。已有 Agent 正文不会被重新翻译。导出报告时，标题、表头和状态使用当前界面语言，保留原始正文。

CLI 示例：

```sh
wukong --locale en-US --help
wukong --locale zh-CN analyze ./sample --output-language en-US --task "分析输入格式"
wukong --locale en-US report RUN_ID --output ./report-en
```

`WUKONG_LOCALE` 与 `WUKONG_OUTPUT_LANGUAGE` 可提供命令行默认值。恢复任务仍使用任务创建时保存的输出语言。切换语言不需要重建或下载 Docker 镜像。
