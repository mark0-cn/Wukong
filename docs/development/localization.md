<a id="多语言维护"></a>

# Localization maintenance

English · [简体中文](localization.zh-CN.md)

<a id="资源与边界"></a>

## Resources and boundaries

Wukong supports `zh-CN` and `en-US`. Frontend/Rust share Fluent resources in `locales/<locale>/*.ftl`, embedded by Vite/`build.rs`. Runtime neither calls translation services nor downloads language packs.

- `common`: app title, language settings, native dialogs.
- `workspace`, `settings`, `updates`: desktop UI.
- `tools`: display messages for catalog tools/legacy aliases.
- `errors`: controller status, notices, expected errors.
- `cli`, `report`: command help and readable exports.

Tool names, JSON fields, status enums, paths, addresses, payloads, hashes and raw evidence remain unchanged. Original tool text, existing model prose and third-party errors remain original. App errors use `{code,args,detail}`, with raw diagnostics in `detail`. Business error types must not be inferred from translated text/Chinese substrings. Docker diagnostic text matching supplies advisory troubleshooting categories only; it does not replace typed errors or prove the root cause.

React subscribes to language changes; language is not a component key and does not save workspace configuration, change epochs or restart tasks. Do not cache translations during module initialization; resolve status/settings labels at render time. Use Fluent arguments for complete sentences rather than splitting grammar across fragments.

<a id="偏好与任务输出"></a>

## Preferences and task output

`preferences.json` lives at the fixed user-data entry ([installation](../user/online-install.md)), independently of model configuration, task database and movable cache. Native commands atomically replace a same-directory temporary file; frontend applies preferences only after successful save.

UI follows the system by default. `zh`, Simplified Chinese and Singapore Chinese map to `zh-CN`; others fall back to `en-US`. Changing language retains message/settings/task drafts.

New-task output can be Chinese, English or automatic. Automatic is a bounded default rule: Chinese characters in the initial task select Chinese, otherwise ASCII letters select English, otherwise current UI language (system language for CLI). It makes no extra model request and cannot reliably classify mixed language; choose explicitly when needed. A concrete `output_language` is saved in task configuration. Resume reads the snapshot; old tasks do not follow changed preferences. Legacy tasks missing the field retain the old policy: follow the user's language, default Chinese if unspecified.

`agents::preamble` injects one output policy after stable role/tool/Skill prefixes. Explicit task language requests win. Skill method bodies remain single-source, without language-specific methods/permissions. Prompt tests check injection/recovery invariants, not actual model obedience; current tests make no paid requests.

<a id="cli报告和安装器"></a>

## CLI, reports and installers

- `--locale auto|en-US|zh-CN` controls help, app error wrappers and readable reports, defaulting from `WUKONG_LOCALE`; explicit CLI wins.
- `--output-language en-US|zh-CN` or `WUKONG_OUTPUT_LANGUAGE` controls new tasks; configuration `output_language` is also supported. Resume ignores new defaults.
- Commands/argument names stay stable. Raw Clap diagnostics/technical metadata may remain English. Structured JSON and canonical tool/Skill protocol descriptions are not rewritten by display language. Human catalogs support explicit Chinese export; see maintenance.
- Desktop exports use current UI language; CLI uses `--locale`. Headings/status/columns translate, existing content/evidence do not. Tests compare snapshots, replay data and raw bytes across languages.
- NSIS offers English/SimpChinese independently of app preference. AppImage preparation/recovery uses UI language; desktop entries include Chinese `Comment`. Both languages share installers, version, signatures, manifests and tool images.

<a id="新增或修改翻译"></a>

## Adding or changing translations

1. Add stable readable message IDs to both resource directories with matching argument names/counts. Never use user content as an ID.
2. Run `node scripts/check-locales.mjs --write` to regenerate frontend `Messages` types.
3. Use `t` in UI and `i18n::Message` at native boundaries; evaluate for display only and retain complete originals in their original storage.
4. Run `npm run check:locales`, `npm test`, `npm run build` and relevant Rust tests; interaction changes also run `npm run test:e2e`.

Resource checks run before npm tests/typecheck/build, rejecting syntax errors, duplicate IDs, mismatched keys/arguments and stale generated types. Tool-catalog tests check bilingual titles. CI exercises desktop features and bilingual browser regressions on Windows/Linux.

Browser tests simulate Tauri commands, not real WebView, NSIS language selection, Windows Docker Desktop or cross-version upgrades. Installer validation follows [desktop updates](desktop-updates.md) in isolation. Human documentation follows the separate [English/Chinese documentation policy](documentation.md).
