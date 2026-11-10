# Exit-time consumers of a write primitive

Use when a confirmed write may affect a callback consumed at termination.
Identify the mapped libc/loader, ABI, registered objects and exact termination
path. `exit`, `_exit`, `quick_exit`, an exception and returning from a function
do not share all cleanup behavior.

## Prove the consumer

1. Trace which path the input can reach, including early failures and whether
   main ever returns. Observe registration and invocation order when relevant.
2. Inspect the build's exit list, callback flavor, function signature, argument,
   index/count, linked objects, TLS destructors and loader finalizers only as
   needed. A writable global is useful only if that execution later reads it.
3. For encoded pointers, derive the transformation and guard provenance from
   this ABI/build. A stack canary is not a pointer guard. A mangled callback is
   not a callable raw address. An assumed known-plaintext function also needs
   verified library/load-bias identity.
4. Check object liveness, synchronization, permissions and the actual indirect
   call constraints. In glibc, obsolete allocator hooks are not a replacement
   for proving the active dispatch path; musl has different structures.
5. Test a bounded distinguishable callback effect and a wrong-trigger control.
   Check native execution with the original protections and service environment.

Do not upgrade a list overwrite, changed guard or crash into executed behavior.
Preserve the original and modified bytes, derived addresses, registration and
termination observations, exact input and runtime profile. Hand a reproducible
predicate to an independent verifier. Report a missing disclosure/guard/trigger
as a prerequisite gap rather than trying unrelated named techniques repeatedly.

The exit fixture exercises real atexit dispatch and the `_Exit` control. It does
not demonstrate corruption of a libc exit list or pointer-guard recovery.
