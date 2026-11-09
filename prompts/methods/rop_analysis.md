# ROP and control-flow analysis

Use when the cause supports return-address or indirect-control-flow manipulation.
The output is a demonstrated primitive or requested minimal PoC, with unresolved
preconditions explicit. Load `pwn` for shared runtime/address evidence rules.

## Establish control and call semantics

Record architecture, word width, endianness, calling convention, input framing,
length limits and bad bytes. Confirm overwrite extent using original cyclic bytes
and a crash snapshot. Inspect the saved return slot if PC remains at a failing ret.
An arbitrary fault address is not automatically a usable control-flow target.

For each candidate gadget, record module hash, module-relative offset, runtime
address, exact instructions, stack consumption, clobbers and memory operands.
Verify candidates against the actual mapped bytes. Account for the full chain,
including function arguments, cleanup, pivots and the next continuation.

Measure SP at the call boundary. Alignment depends on ABI and consumed stack
words; an extra ret is conditional, not a universal glibc workaround. Diagnose
an alignment fault from the actual instruction and operand. Check relevant canary
code, executable permissions and observed CET state before claiming a viable path.

## Choose a path by its prerequisites

| Path | Evidence required before assembling a chain |
| --- | --- |
| Existing target function | Correct runtime address, reachable calling convention, arguments and a unique observable predicate. PIE requires an established load bias. |
| Library call | A real disclosure or other address knowledge, identity of the mapped library, symbol offset and ABI state. A page-aligned candidate is insufficient. |
| Stack pivot | A controlled readable stack region, valid subsequent words, pivot semantics and preserved access permissions. |
| CSU sequence | Actual matching instructions in this binary; compiler/startup objects may omit them. Check indirect-call target, loop/cleanup constraints and argument truncation. |
| Dynamic resolution | Dynamic target and compatible resolver entry, valid staged records and writable storage, actual loader/relocation/version behavior. Inspect helper reliability warnings; a generated payload is not proof. |
| Signal return | Reachable syscall path, ability to invoke the architecture's sigreturn operation, a valid accessible frame and the kernel's frame/CET constraints. A syscall gadget alone is insufficient. |

Seccomp may reject a syscall even when a register setup is correct. Record the
observed syscall result or fault and separate policy failure from gadget failure.
Do not disable a target protection to count an otherwise failing chain as success.

## Close with measured evidence

Use the smallest experiment that distinguishes stack control, a reached function
and the requested effect. Preserve disassembly, SP/register snapshots, input hash,
transcript and a meaningful no-trigger or wrong-target control. Confirm each new
process's address assumptions rather than reusing an old ASLR-dependent payload.
Return the result to the primary assignment with a reproducible verification
predicate and limitations; leave independent certification to the verifier.
