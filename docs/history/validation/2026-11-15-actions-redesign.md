# Actions consolidation acceptance — 2026-11-15

English · [简体中文](2026-11-15-actions-redesign.zh-CN.md)

## Source and scope

[PR #30](https://github.com/mark0-cn/Wukong/pull/30) integrated the Actions redesign at `08886cfde1a6929f1b8e90bab2be13ded2fb9701`. The repository now has five operator entries (Checks, Acceptance, Version, Release, Maintenance) and three internal workflows (Policy, Client, Installer). GitHub-managed Dependabot Updates and Dependency Graph remain enabled. See the [entry-point guide](../../development/github-maintenance.md#actions-entry-points).

The PR merged through native auto-merge at 2026-11-15 04:05:57 UTC, with zero reviews, after its required checks passed. The common ruleset still requires a PR, strict updates, resolved conversations, and GitHub Actions `summary`/`commit-policy`; it has no bypass actors. A separate ruleset still requires one approval and dismisses stale approvals for non-admins, with only the administrator role exempt. This verifies administrator auto-merge and the non-admin configuration; no separate non-admin account was used for a live merge test.

## Observed checks

Local validation passed: 161 automation tests, actionlint, version consistency, PowerShell online-package tamper and installer transaction fixtures, commitlint, bilingual document pairs/links/fingerprints and diff checks. These local installer fixtures do not install over the operator's application or call real Docker/model APIs.

| Run | Observed result |
| --- | --- |
| [PR Checks](https://github.com/mark0-cn/Wukong/actions/runs/43673888493) | Passed Windows/Linux, both Ubuntu APT matrices, actual NSIS install/uninstall, acceptance-gate and summary |
| [Policy after a PR body edit](https://github.com/mark0-cn/Wukong/actions/runs/43674111084) | Passed; the existing Checks run continued without restart |
| [Master Checks](https://github.com/mark0-cn/Wukong/actions/runs/43675255928) | Passed on the exact merge SHA; ordinary integration skipped release-request |
| [Version preview](https://github.com/mark0-cn/Wukong/actions/runs/43675373382) | Passed the v1.0.1 dry run; App token and remote branch/PR creation steps skipped |
| [Maintenance cache](https://github.com/mark0-cn/Wukong/actions/runs/43675255874) | Passed verified dependency-cache warmup |
| [Maintenance cleanup](https://github.com/mark0-cn/Wukong/actions/runs/43675376381) | Passed with apply=false; cache/toolchain jobs skipped |
| [Maintenance toolchain](https://github.com/mark0-cn/Wukong/actions/runs/43675719567) | Passed report generation; no dependency upgrade or publication |
| [Release inspect](https://github.com/mark0-cn/Wukong/actions/runs/43675869554) | Passed read-only inspection of prior successful master CI 43667394398; no tag credentials or tag |
| [Release rehearsal](https://github.com/mark0-cn/Wukong/actions/runs/43676645599) | Failed container acceptance and final summary; exact master Checks reused; image, three clients and both native installers built successfully; package/publish skipped |
| [Additional Windows worker installation](https://github.com/mark0-cn/Wukong/actions/runs/43675380212) | Failed startup qualification; acceptance-gate also failed, as required |

## Windows worker gap

The additional `Acceptance/windows-worker` run used Windows Server 2025 build 10.0.26100, runner image `windows-2025-vs2026` version `20261103.298.1`. Native compilation, unit tests and package contracts passed. Installation then failed service readiness with Windows error 1066: a contained loopback connection timed out, but qualification received no matching live `AppContainerLoopback` WFP drop. This is a failed qualification, not proof of successful network isolation or successful installation.

| Evidence identity | Value |
| --- | --- |
| Source revision | `08886cfde1a6929f1b8e90bab2be13ded2fb9701` |
| Signed package manifest SHA256 | `4e863c78db546ff014c56b520fa355d1e547409f3c47e9ec24782e4ac7406ebf` |
| [Public report artifact](https://github.com/mark0-cn/Wukong/actions/runs/43675380212/artifacts/14145829748) ZIP SHA256 | `66a2df3aae7025e49e806db05a430a32591fe8e55565eb43d3ab32cf696aaa48` |
| Extracted report.json SHA256 | `f8278f22f737dae816c1ba82fa25c81c2bb9b0715e18e1103fb524b912798fb3` |

The downloaded artifact matched its published digest. Its public report records `cleaned_owned_os_resources=true` and `remaining_task_root=false`; ephemeral LSA secrets retire with the disposable VM. It explicitly does not qualify production uninstall, production isolation or a completely disconnected VM. The uploaded artifact has seven-day retention; retain the original downloaded report for later diagnosis.

The worker implementation and installation fixture were unchanged by PR #30. The operator explicitly chose to finish the Actions delivery and retain this separate gap, without modifying worker isolation behavior. Future work should inspect the live WFP subscription/filter correlation, the actual blocking source and hosted event configuration. Do not accept timeout alone, skip qualification or relabel this run as passed. The earlier [Windows 10 v3 acceptance](2026-11-15-execution-contract-v3.md) is bound to a different source/package/environment and does not cover this Server 2025 failure.

## Linux release acceptance gap

The rehearsal tested source `08886cfde1a6929f1b8e90bab2be13ded2fb9701` using candidate image `hlmark0/wukong@sha256:c8a150ec9c071d0f719f8aeb5c22916019eb1158b87a00d72868c2b4e4019487`. The container checks passed sibling network separation, the fixed upstream relay, volume restoration without process/auth authority, and shared launch/archive tests. The subsequent CLI environment doctor failed in [acceptance job 132699545968](https://github.com/mark0-cn/Wukong/actions/runs/43676645599/job/132699545968):

```text
workbench_control.py:54: inventory(root, False, True)
TypeError: doctor() missing 1 required positional argument: 'registrations'
```

The caller supplies three arguments, while `worker_capabilities.doctor(root, trusted, isolated, registrations)` requires four. Both files and the release-image acceptance fixture were unchanged by PR #30. The downloaded job log has SHA256 `057108d83742ba7fe502dd8d9c3d54b06b11d3e5a2fd904e70aacdc901c808fb`. This identifies a runtime caller/signature mismatch; later CLI/package acceptance has not passed. Runtime repair belongs to a separate change, with a regression check of this management entry and a new image-bound acceptance run. Do not substitute empty capabilities or bypass the failed release gate to make this rehearsal green.

## Publication and verification limits

PR checks precede merging; master checks start afterward and must be followed separately. Selected APT/uninstall acceptance now feeds the required summary; opt-in Windows worker installation is a separately selected suite, whose failure remains visible. Cache maintenance is not a merge gate.

This task did not authorize a new official version, remote version PR, tag or Release. The tag-request handshake and accepted-artifact recovery were covered by automation tests; official tagging/publication and publication recovery were not invoked. A release rehearsal may push a public candidate image and upload signed candidate artifacts without changing official version/latest pointers. Hosted installation checks do not establish Windows Docker Desktop or arbitrary production-target acceptance.
