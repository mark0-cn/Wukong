# I/O under syscall constraints

Use when the requested effect depends on a filtered syscall or a jailed service.
Keep the desired effect explicit: existing-data disclosure, file access and
executing a process require different resources and permissions.

## Build an observed resource and policy table

Record architecture and syscall ABI, filter installation timing, thread/process
scope, inherited descriptors, file layout, allowed endpoint and relevant kernel
features. Find which descriptors actually exist and what they refer to; never
assume the useful descriptor is 3, stdin is a socket, or a relative path exists.
Check syscall arguments and actual return values, not just syscall numbers.

`mitigation_profile` reports observed filtering mode, not a policy. Preserve
setup code or extracted filter bytes with provenance. Upload a raw sock_filter
array and use `seccomp_analyze` for bounded disassembly and concrete arch/nr/IP/
argument probes. Supply byte order explicitly when needed. Its hash binds the
artifact, not the installed process policy. Unsupported opcodes, unknown stacked
filters and user-notification decisions remain unresolved. Numeric syscall
values belong to the selected ABI, including x32/compat distinctions.

## Select a feasible experiment

- Existing useful FD: check its access mode/offset and whether the needed read
  and output operations are permitted with these arguments.
- Opening a file: establish path/namespace and the actual open/openat policy,
  then propagate the returned FD and successful byte count; handle errors and
  short I/O. Do not substitute execve/system for a file-read objective.
- IPC or asynchronous I/O: inspect already available peers, SCM_RIGHTS or
  io_uring only when the program and policy expose them. Kernel configuration,
  ring restrictions and operations must be verified; these are conditional
  avenues, not universal seccomp escapes.

Record errno, SIGSYS context and exact I/O. A debugger can alter observations;
replay natively without relaxing the target policy. For a prepared TCP service,
`poc_verify.network` replays a bounded plan through the configured challenge
endpoint. It records fresh connections and raw transcripts, not a reset server
or a confirmed remote runtime hash. Unsupported protocols require an explicit
verification gap. Final security impact still requires an independent verifier.
