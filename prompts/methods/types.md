# Recover data structures

Use for object, structure, union or table layout reconstruction. Require a
known module/ABI and relevant accesses; a decompiler's guessed type is a
candidate, not an observed source declaration.

## Recover layout constraints

- Collect accesses by base object, displacement, access width and use. Track
  allocation sizes and array strides separately from accessed field widths.
- Distinguish inline storage from pointers, values from counters/flags, and
  overlapping union views from unrelated objects that happen to share offsets.
- Cross-check uses at multiple sites and callers. Account for alignment,
  packing, pointer width and ownership when supported by actual evidence;
  leave padding, lifetimes and unused ranges unknown otherwise.
- For runtime checks, identify the instance and load mapping. A captured
  pointer value is not a stable structure offset. Use owned debugger sessions
  only when the configured backend supports the target, then close them.

## Handoff and stopping condition

Provide a layout table (offset, width, proposed type, use, evidence,
confidence), observed object/stride bounds and a candidate declaration with
explicit unknown byte ranges. Report conflicting accesses and alternative
layouts. Stop when the assigned consumers are consistent with the candidate
layout; do not expand into every structure in the binary.
