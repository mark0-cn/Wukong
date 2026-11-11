<a id="仓库自动化"></a>

# Repository automation

English · [简体中文](README.zh-CN.md)

Invoke Python entry points with `python -m` from the repository root, avoiding script-local imports or caller `PYTHONPATH` assumptions.

Native Windows/Linux packaging uses `cargo package-local`; standalone `xtask/` calls `scripts.release.native_package`. See [local installers](../docs/development/releases.md#build-native-installers-locally).

| Directory | Responsibility / example |
| --- | --- |
| `ci/` | Scope selection, summaries, closed-PR cache cleanup, runner disk preparation |
| `release/` | `python -m scripts.release.cli check`; versions, packaging, publishing, artifact reuse |
| `toolchain/` | `python -m scripts.toolchain.fetch_worker_deps`; assets/update checks |
| `dev/` | PowerShell development checks, local GUI deployment, standalone workers |
| `_lib/` | Repository location, version/hash rules, GitHub API and shared implementation |

PowerShell wrappers locate the repository; user paths resolve against the invocation directory. Tauri platform configuration generates native installers. `installer_artifact` install/uninstall smoke checks run only on isolated runners, not over existing developer installations. `packaging/windows` scripts support retained development fixtures. Script/installer tests: [tests/automation](../tests/automation); real image acceptance: [tests/acceptance](../tests/acceptance).

```sh
python -m unittest discover -s tests/automation -p 'test_*.py'
```

Generate current UI previews with `node scripts/dev/capture-readme.mjs`; prerequisites and sample-data boundaries are in [screenshot maintenance](../docs/development/maintenance.md#readme-screenshots).
