# FILE stream primitives

Use after a demonstrated write can reach a live stream or a supplied task asks
about FILE behavior. Identify the actual libc family, hash, ABI and stream first;
load `musl` for a musl runtime. This method grants no execution permission.

## Start from the available object and trigger

Map the input-to-write path, controlled byte ranges, stream lifetime and the next
operation that consumes it. Separate buffered disclosure, buffered input into a
chosen region, an indirect call and executed behavior. A fake FILE in unused
memory or an overwritten vtable alone demonstrates none of those effects.

For glibc, inspect the build's FILE/FILE_plus and wide-data layouts. Record flags,
buffer bounds, read/write pointers, descriptor, mode, lock, chain and vtable only
where relevant to the selected call path. Validate addressability, permissions,
alignment and the lifetime of every dependent object. Preserve the original
before/after bytes and the exact flush/read/close/exit trigger.

Trace actual source or mapped instructions through each check. Modern glibc
validates ordinary vtables; older string-stream callbacks and newer wide-data
paths have different prerequisites. Do not copy an old offset, assume all wide
paths are unchecked, or equate an ELF CET note with enabled thread protections.
Changing the library or disabling its checks is a separate experiment.

## Bounded experiment and handoff

First test the smallest requested stream effect with a unique, bounded marker
and a no-overwrite or no-trigger control. Confirm the bytes actually read or
written, including buffering and short I/O. Keep a memory-read claim separate
from PC/argument control; a libc abort establishes a rejected state.

Deliver a field/control table, actual call path, runtime identity, input hash,
stream snapshots, native transcript, controls and missing prerequisites. If a
different exit sink is needed, select `exit_handlers` rather than assuming a
stream primitive automatically supplies it. Independent impact certification
still uses `poc_verify` and `verification_record`.

Sources and the deliberately limited FILE-buffer fixture are described in
docs/reference/pwn-knowledge.md and fixtures/pwn/README.md.
