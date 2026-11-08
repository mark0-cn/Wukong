# Recover binary formats

Use for a file, packet or serialized-record layout question. Start from known
input samples, parser locations and the input channel; identify missing samples
or execution capabilities before promising a complete decoder.

## Recover and discriminate

- Follow parser reads and length checks with decompilation and references.
  Record each field's offset, width, byte order, signedness and role. Separate
  constants, lengths, counts, offsets, flags, checksums and payload bytes.
- Test alternative explanations with a small sample set: swap length bytes,
  vary one flag, shorten the header, and distinguish a byte count from an
  element count. State whether lengths include the header or trailing data.
- Preserve rejected samples and observed errors. A parser accepting one
  sample does not establish every optional record variant.
- When execution is needed, stage a harness and sample files as argv inputs.
  Use the actual target's input channel; do not confuse a file path argument
  with stdin. Write decoder outputs in the experiment working directory.

## Handoff and stopping condition

Save a field table with offset/width/encoding/meaning/evidence/confidence,
length relationships, sample hex plus decoded values, and rejection behavior.
A runnable decoder is a useful artifact when supported by the observations;
label guessed fields and untested variants. Stop when the assigned records
are explained and distinguishing cases support the layout, or identify the
specific sample/capability needed to resolve the remaining ambiguity.
