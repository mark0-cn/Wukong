<a id="证据报告与重放包"></a>

# Evidence report and replay bundle

English · [简体中文](report-format.zh-CN.md)

CLI `analyze` (including `--resume`) exports a requested bundle and prints the stored result before checking completion. Only `completed` exits 0; paused, stopped, crashed, failed or otherwise incomplete assessments exit 1 and identify their status on stderr. Inspect the structured result's `status` to distinguish a resumable budget pause from failure. Export success and bundle integrity do not establish analysis completion or semantic correctness.

`wukong::report::export(store, run_id, output)` exports one transactional run snapshot. Export reads and hashes the original target and each evidence artifact; it never executes the sample. A verified staging directory is renamed to the destination only after all payloads pass verification. A new directory is created when needed. An existing destination is accepted only when it is a complete valid bundle for the same run; the previous bundle is staged and restored on failed replacement. Corrupt bundles, symlinks and other run IDs are refused.

`export_with_locale(store, run_id, output, locale)` localizes Markdown/HTML headings, recorded status labels and the bundle README. The compatibility `export` function defaults to English. Desktop export uses the current interface language; CLI export uses `--locale`. Existing prose, snapshot fields, replay parameters, identifiers, hashes and evidence bytes remain unchanged. Only presentation files receive new content hashes; bundle integrity validation still covers all files.

HTML is rendered with an Askama template and automatic HTML escaping. Markdown
uses its own punctuation escaping; neither renderer changes the raw evidence.

The bundle contains:

| File | Contents |
| --- | --- |
| `report.md`, `report.html` | Findings, recorded stages, independent verification status, coverage and accounting |
| `report.json` | Run, agents, checkpoints, findings, verification, coverage, documents, calls, jobs, messages and events |
| `replay.json` | Exact recorded tool arguments, target mapping, evidence mapping, verification environment and conditions |
| `config.redacted.json` | Run configuration with credential fields replaced by `[REDACTED]`; environment variable names remain available |
| `target/sample.bin` | Original target bytes, preserving the run's SHA256 |
| `evidence/XX/SHA256` | Deduplicated original evidence bytes |
| `manifest.json`, `SHA256SUMS` | File sizes and SHA256 digests for all payloads and the manifest |

`verify_bundle(output)` checks every digest and size, the target mapping, the checksum index, relative path containment, symlink escapes and unexpected files. Checksums detect changes; they do not authenticate a publisher. Original evidence bytes are preserved and may contain sensitive program data. Metadata/configuration redaction does not rewrite original evidence.

Findings retain their recorded stage and their evidence-based severity
(critical/high/medium/low/info) with the rationale that justifies it: critical
and high are only accepted at a reproduced-or-better stage, and a bare crash is
at most medium. A recorded `impact_verified` claim is explicitly marked
incomplete unless it has a successful verification record with nonzero valid
attempt counts, evidence, and a separate agent whose role is `verifier`.
Export does not infer code execution from a crash, promote a stage, or invent
missing replay conditions.

Dynamic trace jobs retain `trace/run.json`, `trace/events.jsonl` and the private debugger transcript as original evidence. `trace_compare` writes a separate `trace.comparison` artifact linking both source jobs/evidence, the logical plan and the comparison signature. Completion of the event stream, observation completeness, job settlement and evidence readiness remain distinct; a partial trace can be exported for diagnosis but cannot justify `match`. See [Dynamic function tracing](../user/dynamic-trace.md) for the current backend and limits.

To replay, first verify the bundle, recreate the runtime and dependencies recorded in the calls and verification records, map the original sample path to `target/sample.bin`, import referenced evidence from its `bundle_path`, and replay the recorded tool arguments. Supply credentials through the original environment variable names. Runtime snapshots and third-party engine installations are not embedded in the bundle; their identities and requirements remain part of the recorded metadata.

General-analysis `analysis_verify` records require `checks` (1–64) and `limitations`. Each check binds a claim, expected/observed values, verdict and verifier-owned evidence; the aggregate `supported` verdict cannot include refuted or inconclusive checks. Reproduction checks also bind the exact artifact evidence and the verifier's same-target job with original status evidence and the declared expected exit code; recorded parameters preserve the executed invocation. Coverage explicitly applies only to listed checks, never the whole report. Separate input acceptance, derivation, boundary behavior and command reproduction. These provenance gates do not prove the semantic correctness of a claimed observation.

## Script predicate acceptance

`analysis_verify` resolves the owned job's original status evidence and stores requested arguments separately from resolved bindings. Diagnostics identify all invalid checks by index. A normally completed process with an explicitly expected nonzero exit may support a negative reproduction; signals, timeout, cancellation and lost jobs cannot stand in for normal rejection.

`report.json.snapshot.artifact_validations` and `replay.json.artifact_validations` retain `self_check_v1` acceptance records. They bind the final script and baseline hashes, target, verifier, worker, baseline/driver jobs, original status and full report evidence. Job status shows the bounded acceptance summary and failed scenarios; full cases remain original evidence. Revisions require a new record. Historical scripts without the profile remain unchecked by it.

The [validation method](../../prompts/support/validation.md) defines `--validation-io` (one stdin JSON request, one stdout JSON response), normal `--verify` baseline collection, and the shared evaluator. Python and POSIX Shell are supported; the Shell example explicitly needs Python 3. The driver tests simulated observations inside the configured worker. Its pass does not establish collector wiring, complete claim coverage, target behavior or real occurrence of an injected fault. Those require independent review and real-target evidence.
