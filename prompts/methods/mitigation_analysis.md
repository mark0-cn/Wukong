# Protection declarations, support and runtime state

Use when a proposed primitive depends on RELRO, executable memory, address
randomization, CET, canaries or seccomp. Produce a small evidence table with:
protection, static declaration, environment support, observed process/thread
state, evidence IDs and the implication for this particular path.

Use `binary_inspect` for static target facts and `mitigation_profile` with the
staged target and an owned `session_id` or `interactive_session_id` for runtime
observations. Supply `address` when checking a particular relocation or pointer.
Prefer a stable stopped debugger state; a live process may change mappings while
being observed. A failed observation is unknown, not evidence of no protection.

- RELRO: combine PT_GNU_RELRO with immediate-binding flags for the static label.
  For a chosen slot, inspect its actual permissions after loader initialization.
  Partial RELRO does not imply the entire GOT writable; missing BIND_NOW does
  not prove a selected destination writable.
- NX: PT_GNU_STACK describes a requested stack property. Inspect the relevant
  runtime mapping for the actual write/execute question; do not generalize to
  other mappings or assume a code-generation region exists.
- PIE/ASLR: distinguish executable type from OS policy and a process load bias.
  Establish mappings for the observed session and check new-process variability
  when the claim concerns randomization. A single nonzero base is insufficient.
- Canary: an imported checking symbol does not establish function coverage.
  Verify the actual prologue/epilogue and fail path of the function at issue.
- CET: the ELF IBT/SHSTK note is a declaration. CPU/kernel feature visibility is
  a separate prerequisite. Inspect per-thread x86_Thread_features for actual
  SHSTK state and preserve missing fields as unknown. ENDBR bytes or kernel IBT
  support do not establish userspace IBT enablement. Thread state can differ.
- Seccomp: mode 2 establishes filtering at observation time, not its rules or a
  syscall allowlist. Identify setup code/filter artifacts or observe the specific
  syscall result in the requested experiment. Preserve errno, signal and fault
  context; SIGSYS alone does not explain the entire policy. Debugger behavior can
  differ from untraced execution, so replay the required behavior natively.

Do not alter protections simply to make a hypothesis pass. Record an unsupported
path or a separate changed-environment experiment accurately. Recheck after exec,
loader initialization, thread changes or a new connection/process. Stop once the
protection question relevant to the assigned primitive is answered.
