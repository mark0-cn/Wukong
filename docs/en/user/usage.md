# Desktop and CLI usage

[简体中文](../../user/usage.md) · [Installation](online-install.md) · [Languages](languages.md)

## Start an analysis

Prepare the tool environment in **Tool environment**, then open **Workspace settings** to configure a model provider, model name and credential environment variable. Enter the variable name, not the secret value. Credentials stay in the control process.

Choose **New analysis**, select or drop a local binary, and describe the work you want done. Be specific about behavior, formats, algorithms or security questions. The task's budget limits total tokens, cost, duration, Agent count and collaboration depth.

**Analysis progress** shows public Agent updates and paired tool operations. Expanding an operation reveals exact parameters and results. The collaboration panel lets you select individual Agents and inspect their status. Raw logs, payloads and evidence retain their original bytes and language.

To change language, use **Workspace settings**. Interface language applies immediately; output preferences apply only to new tasks. Existing drafts and selected tasks survive language switches. See [the language guide](languages.md) for automatic selection and CLI overrides.

## Intervene and resume

Select an Agent and send additional information through the composer. Messages are bound to the recipient generation; delivery state appears in the transcript. Select **Assign follow-up** when adding work to a finished Agent. Follow-up work queues until analysis resumes.

**Pause** saves current progress. **Resume analysis** continues from saved configuration and checkpoints; budgets are cumulative limits and may need to be raised. Changing current workspace settings does not replace a resumed task's model configuration or output language.

Closing an active workspace waits for work to pause and save. The close dialog also identifies unsent drafts. Stop an Agent branch only when you intend to stop its subtasks too; existing evidence remains available.

## Findings and reports

**Findings and evidence** separates suspicions, reproduced behavior, confirmed root causes and independently verified impact. A crash or static hypothesis is not proof of successful exploitation. Read the recorded environment, conditions, verification results and evidence references.

**Export** creates JSON, Markdown and HTML reports plus original evidence, target bytes, replay information and an integrity manifest. It does not execute the target. Headings and status use the current interface language; existing findings and evidence are not rewritten. Export refuses an existing destination.

```sh
wukong --locale en-US report RUN_ID --output ./report-en
wukong verify-bundle ./report-en
```

The checksum manifest verifies integrity, not publisher identity. Original evidence is not redacted; the configuration export redacts credential fields and is not a credential backup.

## CLI

```sh
wukong --locale en-US --help
wukong init wukong.toml
wukong --config wukong.toml doctor
wukong inspect ./sample --strings
wukong --config wukong.toml analyze ./sample --task "Recover the input format" --output-language en-US
wukong runs
wukong status RUN_ID
wukong pause RUN_ID
wukong resume RUN_ID --max-tokens 500000
```

Use subcommand `--help` for arguments. `--locale` controls presentation and export language; `--output-language` controls new assessments. `--json` preserves the stable machine protocol and raw event fields. Raw parser diagnostics and third-party technical output may remain in their original language.

## Pwn environments

Tools and challenge services run in sibling containers managed by the host. In **Challenge environment**, import original attachments, build the supplied challenge image, prepare runtime files and start the service. Choose the environment for new tasks. Existing tasks keep their recorded environment.

Tools can connect to `challenge:port` on the challenge's dedicated network. This provides service input/output; debugging requires prepared local files or a dedicated debug container. A tool container's `localhost` refers to that tool container. Keep target binary, libc, loader, working directory and launch conditions consistent; verify final conclusions in the corresponding original challenge environment.

Unknown samples require isolation. `trusted_local` is restricted to trusted development fixtures. The tool container does not receive the Docker socket or model keys. UI language changes do not require an image rebuild.

Advanced method contracts and detailed tool semantics are documented in the canonical [tool catalog](../../reference/tool-catalog.md), [Pwn workbench guide](../../user/pwn-workbench.md) and [implementation contract](../../design/implementation-contract.md).

## Workspace reading and navigation

- Rename a task with the pencil beside its title. Empty names restore the filename. Names are device-local WebView preferences scoped by workspace path and task ID; they do not change instructions, reports, databases or files and are not included in migration bundles. Clearing WebView data removes these names and layout preferences.
- Task history is grouped by date and can be filtered by active or historical/paused work. Panel widths and visibility are remembered locally.
- Open **Model and usage** in the footer to inspect model, tokens, cost and agent count. Connection failures change both the indicator and its text.
- Completed tool operations use compact rows. Large groups collapse successful operations while retaining failed or unresolved operations; every original operation remains accessible. Raw parameters and results load on expansion.
- The composer grows with its content and supports manual expansion. **Add information** does not reopen finished work; **Assign follow-up** explicitly requests more work. Paused assessments still require resuming.
- Reading older progress stops automatic following. The latest-progress button appears away from the bottom and counts newly arriving messages.
- Findings show verification and missing evidence first. A single evidence artifact opens automatically; multiple artifacts use a list. Source, hash and evidence ID have copy controls. Narrow windows use a findings list/detail/back flow.
- Settings are grouped into language/model, budget, storage and advanced configuration. Switching categories or closing settings retains drafts. Active directory changes prevent closing; save configuration drafts before switching directories.
