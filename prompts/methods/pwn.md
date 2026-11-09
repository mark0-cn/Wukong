# Pwn routing and evidence

Use for a requested primitive or minimal PoC after the cause and input path are
known. Load alongside the relevant capability; select only the method needed now.

- Allocation/free lifetime or allocator metadata: `heap_ptmalloc`.
- Saved return address, indirect control flow or gadget chain: `rop_analysis`.
- Attacker bytes used as a printf-family format argument: `format_string`.
- Uncertain RELRO, CET, seccomp or other runtime protection: inspect static and
  live evidence before choosing a path; do not infer enablement from a name.

## Shared preconditions

Pin target/runtime hashes with `runtime_profile`. When a loader and libc are
supplied, use `runtime_prepare` for immutable launch copies and check live
mappings after startup. A library filename or page-aligned subtraction is not a
match. Leaks and derived addresses belong to the observed process/session.

Distinguish file offsets, module-relative offsets, link-time virtual addresses
and runtime addresses. Ghidra queries use module-relative offsets even for
non-PIE. Convert only with verified mapping/load-bias evidence.

Use `crash_snapshot` for a stopped session and `cyclic_offset` for exact pattern
parameters/bytes. A noncanonical return may leave PC at the ret instruction:
inspect the saved slot and fault address before claiming PC control. Canary
imports describe the binary, not every function; inspect the relevant prologue
and epilogue. Do not invent a disclosure primitive from an echo or overwrite.

## Demonstrate the requested effect

Define a predicate tied to the reached code or changed state. A startup prompt,
crash, overwritten slot, changed PC and chosen behavior are different evidence
levels. For protocol targets, drive your own launched instance, obtain its port
from target facts, and preserve exact request/response bytes and session state.

Preserve input bytes, runtime identities, address conversions, observations and
controls that distinguish the hypothesis. `angr_solve` output is a hypothesis
until replayed. Hand the final artifact and predicate to an independent verifier;
report failed or unknown preconditions instead of upgrading partial control.
