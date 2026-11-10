# JIT and self-generated code: dump, then analyze

Use when the target generates code at runtime: a writable-executable mapping
is created, code bytes are written into it and control transfers into the
buffer (or a signal handler is installed for a region the program fills).
Reverse the GENERATED buffer, not the generator.

## Recognize the pattern

- mmap/mprotect with an executable mapping, followed by writes into it and a
  call/jump/return into that address.
- A signal handler registered for a region the program later fills with code.
- Decryption/unpacking loops whose destination is later executed.

## Dump at the transfer point

1. Break at the instruction that enters the generated buffer (the call/jump/
   signal return target), before the first generated instruction runs.
2. Dump the mapped region at that moment (gdb `dump memory` of the buffer
   range). This snapshot is the analysis target.
3. Run the standard static pipeline on the dump: binary_inspect on a staged
   copy when the dump is a well-formed image, or raw disassembly
   (objdump -D -b binary) plus functions_search/xrefs on the addresses.
4. If the code is generated incrementally (later writes depend on earlier
   execution), take one dump per stage at the corresponding transfer points
   and note which stage each dump belongs to.

## Evidence and stopping condition

Keep the generator's identity (mapping addresses, protections), each dump's
address range and moment, and the disassembly evidence. Stop when the
generated code's behavior is understood or when one stage remains
unreachable; state which stage and why.
