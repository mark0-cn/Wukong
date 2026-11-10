# Embedded interpreter primitive reuse

Use when the target embeds a small interpreter — a REPL, calculator or toy VM
whose operations are the attack surface. Enumerate the interpreter's own
primitives as exploit primitives; the win usually reuses them instead of
bypassing the interpreter.

## Enumerate the interpreter as an attacker

1. Map every operation with its memory effect: indexing (array/stack/register
   access with a computed index), arithmetic (division/remainder traps,
   overflow), storage (push/pop, alloc/free of the interpreter's structures)
   and observation (operations that print raw state or lengths).
2. For each operation, ask what a controlled operand breaks:
   - Index out of bounds: computed index into an array becomes an arbitrary
     read/write within the interpreter's own memory.
   - Division trap: division by zero (or an overflow trap) with a registered
     signal handler becomes a controlled trap — inspect what the handler does.
   - Stack underflow/overflow of the interpreter's data stack: pops below
     empty or pushes past capacity change what later operations read.
   - Leak commands: operations that echo raw state expose addresses.
3. Build the exploit from the interpreter's own operations in sequence,
   verifying each intermediate state with a live run; the interpreter's
   feedback (printed state, errors) is the oracle.

## Moving objects and stale references

For a VM with GC, separate logical handles/offsets from native pointers. Record
root encoding, null/sentinel values, tags, object headers, lengths, off-heap
storage and reference widths. Check size_t-to-narrow-offset conversions and
signed/unsigned arithmetic at each boundary. Large allocation arithmetic must
be reachable within the real resource budget.

Observe the heap before and after marking, compaction and reference updates.
Check whether a null root is skipped, overlapping moves preserve source headers,
interior/off-heap references are updated, and reused handles carry a generation
check. Construct the smallest operation sequence plus a no-GC/fixed control.
Do not reuse a pre-GC physical address without revalidation. An out-of-range VM
index or stale logical handle is not a native arbitrary read/write until the
mapped native access has actually been demonstrated.

## Handoff

Deliver the operation sequence, the memory invariant each step establishes
and the observed intermediate states. If a primitive needs preconditions the
interpreter cannot reach, state them instead of speculating about native
memory corruption.
