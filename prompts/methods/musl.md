# musl-specific runtime analysis

Use when file or process evidence identifies musl, including statically linked
targets. Record binary/runtime hashes, ABI, interpreter and provenance; a library
filename or worker installation is not sufficient. The loader and libc may be
the same artifact. Keep that fact distinct from a confirmed live mapping.

## Re-derive implementation-dependent assumptions

- printf: inspect the actual call and implementation. Probe positional arguments,
  mixed argument rules, `%n`, bounded reads and truncation independently. musl
  does implement positional formats; a failed high-index probe does not prove
  all `$` formats unsupported. Preserve exact format bytes and return behavior.
- Canary/TLS: inspect the build's initialization and function checks. Do not
  transplant glibc's guard byte position, TLS offsets or pointer encoding.
- FILE and exit: recover this build's stream fields, callbacks, linked objects,
  locks and termination dispatch. A glibc `_IO_FILE` payload, hook or offset does
  not describe musl. Record who consumes each modified object and when.
- Allocator: establish allocator generation/configuration and exact source.
  musl allocator metadata is not ptmalloc/tcache. If no matching method is
  available, return a bounded lifetime/alias analysis with unresolved layout.

Use supplied artifacts and the prepared launch mechanism, then verify actual
behavior. If preparation cannot preserve the original runtime, record that gap
instead of substituting worker glibc. Static linking also changes symbol and
library-identification expectations. `libc_identify` is an ELF candidate matcher,
not a database proving which runtime a remote service uses.

Deliver implementation-specific observations with hashes, input/transcript,
before/after memory and counterexamples. The musl printf fixture verifies a
small positional-format behavior; it does not certify a musl exploitation suite.
