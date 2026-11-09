# Recover algorithms

Use to identify or reconstruct a transformation, checksum, encoding, numeric
routine or key computation. Require the relevant input/output path and data
representation; signatures and recognizable constants are search anchors,
not proof of an algorithm family.

## Reconstruct exact semantics

- Trace the relevant operations and loop bounds. Record operand widths,
  signed/unsigned interpretation, byte order, overflow, shifts/rotates, lookup
  tables, initialization and finalization. Keep I/O encoding outside the core
  transformation until the relationship is established.
- Write a small independent reference implementation or equations. Match the
  machine-width arithmetic explicitly rather than relying on the host
  language's integer behavior. Avoid executing copied decompiler output as
  the only independent check.
- Compare the binary and reference on discriminating vectors: zero, maximum
  values, high bits, boundary lengths, repeated bytes and differing bytes.
  Preserve mismatches. Matching one output is insufficient to identify a
  standard algorithm or recover hidden parameters.
- Load `tooling` when constraint solving is useful. `angr_solve` currently
  models stdin; use explicit constraints or a suitable harness for other
  channels. Solver outputs still need real-target confirmation.

## Handoff and stopping condition

Provide pseudocode, parameters, exact arithmetic/encoding semantics and a
test-vector table with expected/observed outputs and evidence IDs. Include a
reference implementation artifact when useful. Stop at the requested routine
once the discriminating vectors agree, or state which operation, parameter or
environment dependency remains unresolved.

When the contract requests `algorithms`, return a TaskArtifact named `algorithms` with original evidence_id and structured={kind:"algorithms",rows:[...]}. Every row must cite its own evidence_id; use the structured artifact fields documented in the completion tool schema and project contract.
Each structured row requires input, expected, observed, arithmetic, and evidence_id.
Use differential_test with staged target/reference files and base64 stdin vectors for reusable bounded comparison. Fetch differential-results.json via job_artifacts; mismatches and inconclusive cases must remain visible.
