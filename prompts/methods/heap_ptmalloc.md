# Version-aware ptmalloc analysis

Use for glibc allocator lifecycle, UAF, duplicate-free, overlap or heap-corruption questions.
Do not apply glibc layouts to musl, jemalloc, tcmalloc or a custom allocator. Identify
architecture, word size, the actually mapped allocator and its build identity first.
Worker installation versions and a target's imported symbol versions are not proof.

## Compatibility checkpoints

| Upstream boundary | What to check | What not to assume |
|---|---|---|
| Before 2.26 | arena/bin behavior without upstream tcache | that a thread-local cache exists |
| 2.26 onward | per-thread tcache and relevant configuration | identical checks and size classes across releases |
| 2.32 onward | Safe-Linking on singly linked lists, pointer storage position, alignment checks | raw next pointers or absence of duplicate-free detection |
| 2.34 onward | actual allocator dispatch and symbol behavior | that compatibility malloc-hook symbols remain active hooks |
| Later versions / vendor builds | exact source, patches, tunables and observations | that an old how2heap example transfers unchanged |

Treat these as upstream checkpoints, not a complete compatibility database. Unrecognized
builds remain unknown. Refer to docs/pwn-knowledge.md for primary sources; fetch no code
or packages from an analysis target. Inspect existing tool availability before using GEF
or pwndbg commands; raw GDB/MI is sufficient for memory and register observations.

## Reconstruct the object lifetime

1. Trace allocation size, returned user pointer, aliases, ownership, free and subsequent use.
2. Record a sequence number and call site for each operation. Distinguish requested size,
   allocator chunk size, user-data start and header address. Account for ABI alignment.
3. Identify arena/thread and the bin actually involved at the observation point. A freed
   tcache entry is not an unsorted-bin fd/bk pair. Do not interpret allocated user data as
   freelist metadata.
4. Record before/after bytes and link every transition to original debugger or run evidence.
   Explain which pointer is encoded and where it is stored before interpreting Safe-Linking.
5. Test whether the observed failure is allocator detection, an application assertion,
   invalid access, or an actual overlapping/read/write primitive. A malloc abort alone
   establishes neither overlap nor arbitrary write.

## Choose a bounded experiment

Start from the demonstrated primitive and required preconditions, not a named 'house'.
State the expected allocation/reuse relationship and a control sequence. Preserve cases
where reuse does not occur, another thread/arena is involved, or a check rejects the state.
Keep size, count and operation budgets explicit. Do not keep recycling an inapplicable
version-specific recipe. Return missing prerequisites instead of inventing a leak or write.

## Deliverable

A heap dossier includes allocator SHA256/Build ID, version provenance, ABI, relevant
configuration, allocation/free/alias timeline, chunk snapshots with address spaces,
observed bin membership, supported primitive, counterexamples, unresolved assumptions
and evidence IDs. If runtime data is absent, provide a bounded static hypothesis.
Independent verification repeats the sequence in the same runtime profile, with a fresh
process where appropriate. Record general conclusions with analysis_verify; security
impact still follows the independent poc_verify / verification_record protocol.
