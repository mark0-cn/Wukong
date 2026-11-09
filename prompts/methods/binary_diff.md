# Compare binary versions

Use to explain meaningful changes between two versions. Require identities
and provenance for both sides: hashes, formats/architectures, build hints and
comparable function or behavior evidence.

The built-in binary tools inspect only this run's target. They do not accept
an arbitrary second file path or read another run's evidence IDs. Use a
baseline profile supplied in this run's context/evidence, or an actually
available multi-target adapter. If the second side is missing, request the
specific baseline data through the parent and bound the result accordingly.

## Compare evidence, not raw addresses

- Match functions by combined evidence: names when available, calls,
  constants, strings and structural behavior. Record ambiguous matches rather
  than forcing a one-to-one mapping.
- Normalize module-relative locations and distinguish code movement,
  relocation, compiler/optimization differences and symbol stripping from
  changed logic. Neither a changed hash nor different instruction bytes alone
  establish a behavioral change.
- For a candidate change, compare checks, data layouts and transformations.
  When both versions can actually be executed, replay equivalent inputs in
  comparable environments and retain each side's original outputs.
- Classify a change as added/removed/modified only within the compared scope.
  A security fix is one possible explanation, not the default interpretation.

## Handoff and stopping condition

Provide matched scopes with confidence, before/after behavior or logic,
supporting evidence, build/environment differences and unmatched regions.
Distinguish supplied baseline facts from independently observed facts. Stop
when the assigned difference is explained or identify the missing version,
mapping or experiment that prevents a reliable comparison.

When the contract requests `binary_diff`, return a TaskArtifact named `binary_diff` with original evidence_id and structured={kind:"binary_diff",rows:[...]}. Every row must cite its own evidence_id; use the structured artifact fields documented in the completion tool schema and project contract.
