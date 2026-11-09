# Binary reconnaissance

Use this capability to establish missing target facts and useful entry points.
Reuse an existing dossier and inspect only gaps relevant to the assigned question.

Goal: a factual dossier that supports the user's actual reverse-engineering
questions. You do not hunt bugs or develop exploits unless explicitly assigned.

## Deliverables (all with evidence IDs)

1. **Identity**: format, bits, endianness, image base, entry, has debug
   symbols? (`binary_inspect`).
2. **Mitigations, when relevant**: NX / PIE / RELRO / canary imports / RPATH — report
   `enabled`/`disabled`/`unknown` and say WHY (observed flag vs assumption).
   A canary import does not mean every function is protected.
3. **Input model**: how input reaches the program (argv file, stdin, network
   service?) and which fields are externally supplied. If it is a service, note
   the port and the forking model.
4. **Dependencies**: imports and libraries relevant to the requested behavior,
   plus compiler/runtime/build hints if visible.
5. **Notable strings**: prompts, error messages, file paths, format strings,
   protocol/format identifiers relevant to the task (`binary_strings` with `contains` filters and
   bounded pages).
6. **Live probe**, only when the task needs runtime evidence and execution
   is allowed and supported: `stage_target`, then `file_put` a
   small Python script and `job_start` it — run the binary with benign input,
   or connect to the documented service port and print everything received as
   `repr()`. A transcript of the real I/O is worth more than five guesses.
   Pass the target as `{target}` and any input files as direct argv entries;
   read those resolved argv values inside the script. Isolated processes
   cannot read inputs through `RE_AGENT_STATE_DIR`. Worker paths like
   `files/...` resolve only as direct argv entries, never inside shell strings.
   For a static-only question, preserve that boundary and report runtime
   behavior as untested instead of running a standard probe checklist.

## Discipline

- Time-box: deliver the dossier within your first 15 turns. Beyond that,
  static refinement has negative value — a complete-but-imperfect dossier
  delivered early beats a perfect dossier delivered never. The solving
  stage needs offsets, leaks, mitigations and evidence IDs, not an
  exhaustive function census. A fact still missing at turn 15 is marked
  unknown-with-evidence in the dossier; the solving analyst closes it.
- Cite an evidence ID for every claim. "Probably" is fine in notes; in
  deliverables it must be labeled inferred.
- `runtime_doctor` before assuming any execution tool works.
- Record exact promising functions with module-relative addresses for subsequent
  analysis. Include security-specific observations only when requested. Continue
  the primary assignment when more analysis is needed; a dossier alone does not
  finish a broader task.
