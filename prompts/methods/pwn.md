# Pwn workflow

Goal: turn a confirmed overflow into a verified control-flow hijack with a
minimal, reproducible payload. Load alongside `reverse` or `exploit`; keep the
total skill count small.

## Know the layout before writing bytes

- Buffer-to-saved-RIP offset: start from the reverse handoff, confirm at
  runtime. Generate a cyclic pattern (pwntools `cyclic(N)` in a `file_put`
  script), feed it via `job_start` or `crash_replay`, observe the faulting RIP
  under `debug_start`/`debug_command`, and recover the offset with
  `cyclic_find`. Cite both the static layout and the runtime observation.
- Non-PIE: use the link-time address from `function_decompile` directly.
  PIE/ASLR: leak a pointer or use partial overwrites — never guess bases.
- A global canary import does not mean THIS function is protected: check the
  actual prologue/epilogue of the vulnerable function before assuming no
  canary.

## The stack-alignment trap (glibc 2.35+, incl. Ubuntu 24.04)

Entering a function via a corrupted `ret` leaves rsp 8 bytes off the ABI's
call-entry alignment (a `call` would have pushed a return address). Modern
glibc's `system()` (posix_spawn path) and several libm/libc helpers execute
`movaps` against the stack and SIGSEGV on misalignment: the payload reaches
the function, prints its first output, then dies INSIDE libc. Fix: prepend a
single `ret` gadget before the target address (padding + ret_gadget +
target). Find gadgets with ropper (see tooling); one `ret` is enough to
realign. Confirm the fix with a real run, not by reasoning about the crash
site.

## Choose the win marker from the target's own output

The verification predicate must be an observation that ONLY the reached
function produces — decompile it and cite the exact string. A prompt printed
by the vulnerable function itself is not a win marker. Record the exact
expected stdout/stderr bytes.

## The staged checklists (follow these step by step; each step has a
verification artifact)

### ret2win
0. Dossier facts: non-PIE win address (banner/leak), input channel, buffer.
1. Offset empirically: cyclic(200) -> crash -> cyclic_find; confirm with
   N 'A's crashing and N-8 not.
2. Alignment: one plain ret gadget before win; run the naive no-ret
   attempt and OBSERVE it fail before adding the ret.
3. Payload = padding + ret + win.
4. Controls: no-overflow, no-ret, wrong-target — all must fail.
5. 3/3 reproductions against the real service; save the payload and
   transcripts as evidence, then agent_finish.

### ret2libc with canary
0. Dossier facts: buffer size, canary offset, saved-RIP offset, non-PIE
   base, the leak source (banner %p / dlsym reference).
1. LEAK THE CANARY EMPIRICALLY: padding that reaches past the canary,
   read the echo/leak, extract the 8 bytes at the canary offset. Never
   assume a canary from static analysis.
2. COMPUTE THE LIBC BASE: leak - symbol_offset (symbols from the worker's
   own libc via pwn.ELF('/lib/x86_64-linux-gnu/libc.so.6')).
3. ASSEMBLE: padding + canary + saved-RBP junk + pop_rdi + binsh + system
   (or a write-wrapper syscall path when system misaligns).
4. Controls: no-overflow, wrong-canary, wrong-base — all must fail.
5. 3/3 reproductions -> agent_finish with evidence IDs.

### Live service connection (protocol challenges)
1. Find the bind in disassembly: socket()/bind()/listen() and the port
   constant (or a getsockname/printf of the port). Do not port-scan.
2. Run the binary yourself in the background (exec/py_run), then connect
   with pwntools remote('127.0.0.1', port). Drive your OWN instance: any
   other running instance may differ in state, user or file layout.
3. Capture everything as repr(); derive the protocol state machine.
4. The flag comes from the service's own output; the connection
   transcript IS the evidence.

## Heap exploitation basics (tcache era)

When the target uses heap allocations, keep these facts and primitives in
mind; verify each layout observation against the actual allocator version:

- Chunk layout: size/prev_size header precedes user data; freed chunks
  store forward/back pointers in the first 16 bytes of user space. glibc
  2.26+ keeps small chunks in per-thread tcache (LIFO, no integrity checks
  beyond a count), 2.35+ adds safe-linking (pointers stored XOR'd with the
  heap address).
- The most common primitives, in order of usefulness:
  1. tcache poisoning: corrupt a freed chunk's next pointer so the next
     malloc of that size returns an arbitrary address (overwrite a target
     pointer, e.g. a function pointer or GOT entry when writable).
  2. Use-after-free: a dangling pointer is reused after free — turn it
     into a double-free or an overlapping chunk to build a poisoning
     write.
  3. Double-free (or overlapping free) of the same chunk, then reallocate
     over it to forge the forward pointer.
- Debug with the worker's gdb (gef: `heap chunks`, `heap bins`, `tcache
  bins`) rather than reasoning from prints. Confirm the allocator version
  from the worker libc, not the target's build hints.

## Format-string exploitation

When user-controlled data reaches a printf-family format argument:

1. Locate the offset: send `%p` probes (e.g. `%N$p` for N=1..20) and find
   which positions echo your input bytes — that N is your write index.
2. Choose the WRITE TARGET first: partial RELRO -> a GOT entry you can
   trigger afterwards; full RELRO -> a writable data pointer used by a
   later call, or a saved return address on the stack (the probe leak
   reveals stack addresses).
3. Write with `%N$hn` (2-byte) or `%N$n` chunks, splitting the target
   value into ascending short writes; pad with `%<len>c` before each
   `%hn`. Recompute the length after every chunk.
4. Trigger the overwritten target and observe the win marker; record the
   exact final format string as evidence.

## ROP when gadgets are scarce

- ret2csu: x86-64 binaries linked with __libc_csu_init expose a universal
  gadget (the pop-rbx..r15 + call [r12+rbx*8] sequence) that sets rdi/rsi/
  rdx and calls through a chosen pointer — enough for write/syscall chains
  when pop-gadgets are missing. Confirm the sequence exists before
  depending on it.
- ret2dlresolve: resolve a symbol at runtime through the dynamic linker
  (pwntools `Ret2dlresolvePayload`) when the PLT lacks the function you
  need — no libc base required.
- SROP: one `syscall; ret` gadget plus a crafted signal frame sets every
  register at once; prefer when a single gadget and a readable stack are
  all you have.

## Partial RELRO GOT overwrite

When BIND_NOW is absent (partial RELRO), GOT entries are writable: an
arbitrary-write primitive (overflow, format string, uninitialized index)
can repoint a later-called import to a chosen address. Chain: find the
write -> pick the GOT entry of a function called AFTER the write -> set it
-> trigger. Full RELRO removes this option; use the format-string or
heap paths instead.

## Verify with the real binary

- `angr_solve` (see tooling) produces a hypothesis input; replay it against
  the real target with `job_start`/`crash_replay` and cite BOTH the solver
  job and the replay job.
- "RIP points where I want" is not control-flow evidence and not code
  execution: only an actual run demonstrating the win marker counts.
- Hand the final payload to a separate `verifier` agent with the predicate.

Save payload bytes, offset, gadget/function addresses and execution evidence IDs.
Return to the primary assignment; only a separate verifier can certify the impact.
