<a id="文档维护"></a>

# Documentation maintenance

English · [简体中文](documentation.zh-CN.md)

<a id="范围与目录"></a>

## Scope and layout

English is primary. Human Markdown documentation uses the existing canonical path, with a complete Chinese sibling: `README.md` / `README.zh-CN.md`, or `docs/user/usage.md` / `docs/user/usage.zh-CN.md`. This includes AGENTS, developer references, component READMEs and historical narratives. Every pair has reciprocal language links; ordinary document links stay in the reader's language. Matching explicit anchors preserve links across translated headings and existing bookmarks.

`README.en.md` and `docs/en/user/*.md` are compatibility navigation pages, not separate copies. Do not add another language directory or maintain abbreviated translations there. The documentation checker enumerates tracked and nonignored new Markdown files, so new guides also require pairs.

Runtime `prompts/*.md` are executable instruction assets and remain single-source. Generated report prose follows export locale; raw logs, evidence, configuration examples, identifiers and legal notices keep their original form. These are not duplicate human guides. Translate explanations, not command names, schema keys, addresses or evidence bytes. Historical dates, counts and limitations remain historical; never rewrite raw evidence to claim new acceptance.

<a id="更新与核对"></a>

## Update and review

1. Verify the behavior against code, tests and requirements. Compare both existing languages before editing so unique information is not lost.
2. Update both files in the same commit. Cover the same prerequisites, steps, options, limits, failures, trust boundaries and validation claims. Examples may translate user-facing prose but must preserve command/schema meaning.
3. Check every section and technical value manually. Matching headings and hashes do **not** prove semantic equivalence.
4. Run `python -m scripts.ci.check_docs`. Fix missing pairs, language links, local paths/anchors and section mismatches.
5. After reviewing the changed pairs, record their current contents with `python -m scripts.ci.check_docs --record`. Review the resulting `docs/translations.json` diff, rerun the check and `git diff --check`, and commit both versions plus their review record.

The review record stores normalized UTF-8 text hashes for each pair. A changed file or a new/deleted pair fails until reviewed and recorded; `--record` is an acknowledgment, not machine translation or automatic semantic review. It checks structure before writing and does not make incomplete pairs pass. CI runs the same read-only check, including documentation-only PRs. Remote websites are not fetched by this offline check; verify changed external references separately when their content matters.

<a id="生成目录"></a>

## Generated catalogs

Tool and Skill catalogs are generated from registered metadata. English is reproducible by default, independent of OS locale; explicit `--locale zh-CN` selects Chinese Markdown. `--json` remains identical across languages. Neither command opens workspace configuration/state or connects to a model.

```sh
cargo run -- tool-catalog > docs/reference/tool-catalog.md
cargo run -- --locale zh-CN tool-catalog > docs/reference/tool-catalog.zh-CN.md
cargo run -- skill-catalog > docs/reference/skill-coverage.md
cargo run -- --locale zh-CN skill-catalog > docs/reference/skill-coverage.zh-CN.md
cargo test --test tool_catalog
```

Use UTF-8 output redirection (PowerShell 7 or a UTF-8-aware writer). Chinese tool descriptions live in `locales/zh-CN/tool-catalog.json`, paired with exact registry source text. Changed/new/removed registry entries fail Chinese generation until reviewed. This translation data is used only for human catalogs, never model schemas, permissions or execution. Regenerate both files, review the translated descriptions and update the documentation review record. Skill source links select the corresponding Chinese guide; tool IDs, roles, fixture IDs and protocol metadata stay canonical.
