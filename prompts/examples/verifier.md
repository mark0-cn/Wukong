# Worked example: independent format verification

The producer proposes this fixture layout: RECD magic, a one-byte version,
one-byte flags, little-endian two-byte payload length, payload and one XOR
checksum byte. This is a claim to test, not a trusted tool result.

1. Read its evidence and confirm the exact target/input channel. Stage the
   target and file inputs for a separate job_start harness. The harness gets
   resolved paths as argv; it does not read the worker's private state root.
2. Use the concrete sample hex `524543440101030041424340`. The expected
   observation is exit 0 and `version=1 flags=1 length=3`.
3. Change the length bytes from 0300 to 0003: expect exit 4 / `ERR length`.
   Change the final byte from 40 to 41: expect exit 5 / `ERR checksum`.
   Shorten the header to five bytes: expect exit 2 / `ERR truncated`.
   These controls distinguish length byte order, checksum validation and
   minimum-size validation from merely recognizing the magic string.
4. Capture the real exit codes and outputs. If they match, note_save the
   claim, test inputs, expectations, observations, controls, target hash,
   environment and evidence IDs. Finish with a pass restricted to these
   records and controls. A mismatch is reject or inconclusive with details.

No finding ID is needed for this general-analysis check. For an actual
security finding, use its finding ID with poc_verify and verification_record;
a matching format or banner alone does not demonstrate a security impact.
