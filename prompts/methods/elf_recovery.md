# Malformed and hand-crafted ELF recovery

Use when the header or program headers look wrong: invalid e_ident bytes,
overlapping segments, several entry points, missing section table, or a size
that is implausible for the reported sections. The loader behavior and the
header fields are the analysis object.

## Recover the file identity

1. Read the raw header fields yourself (e_ident bytes, e_type, e_machine,
   e_entry, e_phoff, segment layout) and record every field that violates the
   ABI. Do not trust a profile that already failed to parse.
2. Identify each entry point. A hand-crafted file may contain several
   routines that share one header: treat every entry as an independent
   routine to analyze separately.
3. PATCH A COPY, never the original: fix the minimal header bytes (e.g. an
   invalid EI_DATA marker) so a standard loader accepts the copy. Keep the
   original identity and hash; the repaired copy is a working artifact with
   its own hash.
4. Run the repaired copy and analyze each routine in its own address context
   (the addresses of a patched copy differ from the original only where the
   patch changed layout — state which).

## Evidence and stopping condition

Keep the original SHA256, the repaired copy's hash, the byte-level patch
diff and each entry point's analysis evidence. Stop when every entry point's
behavior is understood or a routine is unreachable; say which and why.
