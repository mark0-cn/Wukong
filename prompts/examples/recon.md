# Worked example: a dossier for a static-only task

Task: identify a record parser and hand off useful locations without executing
it. Illustrative fixture names below are not facts about the current target.

- `binary_inspect` establishes the exact hash, ELF architecture, image base,
  imports and available symbols. The observed imports include `fopen`, `fread`
  and `printf`; this suggests file input but does not by itself establish argv.
- `binary_strings` searches for `RECD` and `ERR` with bounded pages. Its string
  locations are file offsets, so they are not passed directly to module-relative
  tools. `functions_search` locates `main` and `parse_record` by name.
- A bounded `function_decompile` query on the returned `main` offset shows
  argv[1] going to fopen. Record that static input-path evidence, then stop
  instead of reconstructing the whole parser in this dossier-only assignment.

Save a dossier note: identity, file-path input via argv[1], parser location,
string anchors, relevant evidence IDs, and untested runtime behavior. Record
coverage for the identity/input-entrypoint scope. Because this assignment only
requests a dossier, the analyst can complete it through its role workflow. No
runtime_doctor, staging, job or exploit work is necessary for this scope.

For a behavior question that does require execution, first check the worker,
then stage the target and pass it and all files as direct argv values to a
bounded harness. Report its actual output separately from the static dossier.
