# Language settings

[简体中文](../../user/languages.md)

Open **Workspace settings** to choose the interface language and the default output language for new tasks independently.

| Preference | Choices | Applies to |
| --- | --- | --- |
| Interface language | Follow system, 简体中文, English | Menus, status, tool titles, dialogs, date and number formatting |
| New task output language | Match task language, 简体中文, English | New Agents' public progress, findings and conclusions |

You can change the interface language during analysis. Selected tasks, message drafts, settings drafts and existing results remain intact. Unsupported system languages fall back to English. The Windows installer's language selection is independent of the app preference.

Automatic task language uses a simple default rule: Chinese characters select Chinese; otherwise ASCII letters select English; other input falls back to the interface language. Choose explicitly for mixed-language tasks. An explicit language request in the task takes priority. Existing and resumed tasks retain their saved default language.

Tool identifiers, commands, raw logs, file contents and evidence are never translated. Existing Agent prose is not rewritten. Report headings, columns and status use the language selected at export time while preserving original content.

```sh
wukong --locale en-US --help
wukong --locale en-US analyze ./sample --output-language zh-CN --task "Inspect the input format"
wukong --locale en-US report RUN_ID --output ./report-en
```

`WUKONG_LOCALE` and `WUKONG_OUTPUT_LANGUAGE` supply CLI defaults; explicit options take priority. Resumed tasks keep their saved output language. Language changes never require downloading or rebuilding Docker images.
