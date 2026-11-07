# Evidence report and replay bundle

`re_agent::report::export(store, run_id, output)` exports one transactional run snapshot to a **new directory**. Export reads and hashes the original target and each evidence artifact; it never executes the sample. A verified staging directory is renamed to the destination only after all payloads pass verification. Existing destinations are refused.

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

To replay, first verify the bundle, recreate the runtime and dependencies recorded in the calls and verification records, map the original sample path to `target/sample.bin`, import referenced evidence from its `bundle_path`, and replay the recorded tool arguments. Supply credentials through the original environment variable names. Runtime snapshots and third-party engine installations are not embedded in the bundle; their identities and requirements remain part of the recorded metadata.
