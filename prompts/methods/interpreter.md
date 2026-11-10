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

## Handoff

Deliver the operation sequence, the memory invariant each step establishes
and the observed intermediate states. If a primitive needs preconditions the
interpreter cannot reach, state them instead of speculating about native
memory corruption.
