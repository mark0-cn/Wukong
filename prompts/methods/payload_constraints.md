# Payload bytes through an input transformation

Use when filtering, numeric parsing, encoding, length limits or framing can
change a requested input before it reaches the vulnerable operation.

## Reconstruct the complete pipeline

Write a table from transport bytes through delimiters, parsing, normalization,
allocation/copy and final consumption. For every stage, record byte length,
termination rule, representation, buffer identity/lifetime and evidence. Check
whether validation happens before or after the transformation. NUL is valid
UTF-8 but terminates many C strings; newline framing and short reads are separate.

For Rust, inspect the real entry points and slice/str lengths. A successful
`String::from_utf8_lossy` can borrow valid input, while invalid input can create
an owned allocation containing replacement bytes. Verify the Cow variant and
actual pointer at the indirect call; do not reuse a BSS address after a copy.
An unchecked table index supplies only the observed call target and existing
register arguments, not an arbitrary-call ABI of your choice.

For floating-point input, establish width, byte order, parser format and locale.
Round-trip the exact bits through the target parser. Finite values, negative
zero, infinities and NaN payloads need distinct checks; decimal formatting or a
Python helper does not prove preservation by scanf. Compare bytes, not numeric
equality, when the representation is later executed or reinterpreted.

## Build and validate the candidate

State forbidden bytes, usable lengths, instruction alignment, reachable entry
offsets and architectural preconditions. Check constraints against the final
assembled/packed bytes, including addresses, immediate encodings and padding.
For every new process/connection, re-derive ASLR-dependent values. Network replay
capture/patch steps reject overflow instead of silently wrapping an address.

Save original bytes, transformed bytes, hashes and the discriminating valid/
invalid controls. Emulation and solver output are hypotheses until replayed on
the real input path. A parser rejection, a changed pointer, a crash and executed
chosen behavior are different findings. Stop at the requested verified effect.
