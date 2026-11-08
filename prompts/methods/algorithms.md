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
