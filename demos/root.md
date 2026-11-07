# Worked example: static format reconstruction

User request: "Recover this record header and rejection rules from the binary;
keep the analysis static." The following is an illustrative fixture case.

1. Read analysis notes and the graph. If identity is unknown, create one recon
   child with a narrow identity/input-entrypoint scope and the explicit
   static-only constraint. It reports ELF identity, the `parse_record` symbol
   and `RECD` string evidence; it does not start a live probe.
2. Create a reverse child with `skills=["formats"]`, those evidence IDs and
   the question: recover field offsets, byte order, length/checksum logic and
   error branches. Reuse the dossier instead of repeating recon.
3. The reverse child saves the field table and distinguishes static parser
   deductions from runtime observations. If a field remains ambiguous, send
   a targeted follow-up to that child; wait for its new result. Do not create
   a vulnerability finding for an ordinary length field.
4. Reconcile coverage and answer each original question. The conclusion is
   "layout inferred from these parser branches; dynamic rejection behavior
   untested under the requested static-only scope", with evidence references.
   Finish the scan when the requested static questions are answered.

If the user later asks for runtime confirmation, assign a verifier the table,
input channel and distinguishing test cases. That is a new scope decision;
it does not retroactively turn static deductions into executed observations.
