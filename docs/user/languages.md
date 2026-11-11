<a id="语言设置"></a>

# Language settings

English · [简体中文](languages.zh-CN.md)

Open **Workspace settings** to choose independently:

| Preference | Choices | Applies to |
| --- | --- | --- |
| Interface language | Follow system, 简体中文, English | Menus, status, tool titles, dialogs, dates and number formatting, immediately |
| New task output language | Match task language, 简体中文, English | New Agents' public progress, findings and conclusions |

You can change interface language during analysis. Message and configuration drafts, selected tasks and existing results remain intact. Unsupported system languages fall back to English. NSIS installer language does not change application preferences.

Automatic task language uses the initial description: Chinese characters select Chinese; otherwise ASCII letters select English; other input uses interface language. Choose explicitly for mixed-language tasks. An explicit language request in the task takes priority. Existing and resumed tasks retain their saved default language when preferences change.

Tool identifiers, commands, raw logs, file contents and evidence are not translated. Existing Agent prose is not rewritten. Report headings, columns and status use the interface language at export time, preserving original content.

CLI examples:

```sh
wukong --locale en-US --help
wukong --locale zh-CN analyze ./sample --output-language en-US --task "分析输入格式"
wukong --locale en-US report RUN_ID --output ./report-en
```

`WUKONG_LOCALE` and `WUKONG_OUTPUT_LANGUAGE` supply CLI defaults; explicit options take priority. Resumed tasks retain the output language saved at creation. Language changes do not require rebuilding or downloading Docker images.
