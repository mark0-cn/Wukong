<p align="center"><img src="icons/icon.png" width="88" alt="Wukong logo"></p>

# Wukong

[简体中文](README.md) · English

Wukong is a multi-agent workspace for binary reverse engineering, behavior analysis and independent verification. Its Rust/Rig core provides both a Tauri/React desktop and a CLI. A lead Agent coordinates specialists, who return traceable conclusions, original evidence and reports.

| Capability | What it provides |
| --- | --- |
| Binary analysis | ELF, PE and Mach-O profiles and strings; Ghidra functions, decompilation and cross-references |
| Collaboration | Analysis and independent verification roles, shared notes, checklists and evidence |
| Dynamic analysis | Isolated GDB/MI, persistent processes, interactive sessions, AFL++ QEMU, angr, crash replay and input minimization |
| Persistence | SQLite state, checkpoints, resumable work, concurrency and token/cost budgets |
| Evidence delivery | JSON, Markdown and HTML reports, original samples, evidence and integrity manifests |
| Languages | English and Simplified Chinese interfaces; independent per-task output preferences |

Dynamic analysis currently targets **Linux x86_64 ELF**. PE and Mach-O static profiles are supported; native Windows DbgEng and macOS LLDB dynamic backends are not implemented.

## Get started

Download the Windows x64 `-setup.exe` or Linux x64 `.AppImage` from [GitHub Releases](https://github.com/mark0-cn/Wukong/releases). The client downloads its matching Docker tool image during setup. Updates are managed in **Updates and environment**.

Follow [installation and updates](docs/en/user/online-install.md), then [the usage guide](docs/en/user/usage.md). Choose your preferred interface and task language in **Workspace settings**; see [language settings](docs/en/user/languages.md).

To build from source, install the Rust toolchain specified by `Cargo.toml`, a Node.js version satisfying the locked dependencies, and [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/):

```sh
git clone https://github.com/mark0-cn/Wukong.git
cd Wukong
npm ci
npm run desktop:build
```

For development, use `npm run desktop:dev`. `cargo run -- --locale en-US --help` lists CLI commands. `cargo run -- init wukong.toml` creates configuration without overwriting an existing file.

Configure a supported model provider and reference credentials through an environment variable. Credentials stay on the host. Dynamic analysis requires a configured Docker or SSH worker. `trusted_local` is for trusted development fixtures and is not a fallback for unknown binaries.

## Environment model

The host manages sibling tool and challenge containers. Each challenge has its own network and runtime libraries. Tools connect to a service through `challenge:port`; a tool container's `localhost` refers to itself. Debugging may require extracted runtime files or a dedicated debug container. Final verification uses the original challenge environment.

Language changes share the same client installation, update manifest, version and image digest. They do not change evidence, model credentials or challenge data.

## Documentation and development

- [Installation and updates](docs/en/user/online-install.md)
- [Desktop and CLI usage](docs/en/user/usage.md)
- [Language settings](docs/en/user/languages.md)
- [Architecture](docs/design/architecture.md) and [maintenance](docs/development/maintenance.md) (Chinese)
- [Tool catalog](docs/reference/tool-catalog.md) and [Skill coverage](docs/reference/skill-coverage.md)
- [Localization maintenance](docs/development/localization.md) (Chinese)
- [Contributing rules](AGENTS.md), [release process](docs/development/releases.md) and [issues](https://github.com/mark0-cn/Wukong/issues)

Use `npm test`, `npm run build`, `npm run test:e2e`, `cargo fmt --all -- --check`, `cargo clippy --locked --all-targets -- -D warnings` and relevant Rust/worker tests. Browser tests use a simulated native boundary; real container and installer acceptance are separate.

Licensed under [Apache-2.0](LICENSE). Wukong's architecture takes inspiration from [Strix](https://github.com/usestrix/strix) and builds on Rig, Tauri, React, Fluent and the analysis tools documented in the repository.
