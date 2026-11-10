# Worked example: distinguish buffered disclosure from PC control

An assignment supplies a confirmed bounded overwrite of a live stream's pointer
fields and asks whether chosen data can be emitted. Pin the target and libc,
inspect the actual FILE layout and identify the reachable flush call first.

Use a short unique marker in a known readable region. Compare an unchanged
stream, the pointer-change candidate without a flush, and that candidate with
the real trigger. Preserve the write bounds, pointer snapshots, file/pipe bytes
and native exit result for each fresh process. A rejected lock/vtable state is
a negative result, not a partial shell.

The original fixture in fixtures/pwn/src/stream.c intentionally supplies pointer
control while preserving a legitimate stream. Its scope is emitted bytes after
flush. It does not supply a root-cause exploit, a forged vtable, or PC control.
Keep those as separate claims. If the measured predicate succeeds, hand the
artifact and runtime conditions to a separate verifier, then report only that
stream effect. Do not extrapolate it to arbitrary code execution.
