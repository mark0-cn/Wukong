# Prepared Pwn environments

Use when a challenge has a Dockerfile, supplied libc/loader, service endpoint or a
prepared workbench. Begin with `runtime_doctor`; its `workbench` field identifies
the worker-relative LaunchSpec, target and service mode. The host operator manages
`wukong env` and `wukong challenge` commands; never install Docker in a target
worker or request the host Docker socket.

## Local experiments

Pass the same `launch_spec` to `debug_start`, `interactive_start`, `crash_replay`,
`input_minimize` and `poc_verify`. Supply the declared target path (or an identical
staged target). Do not override args/env separately. For `argv_file`, supply an
input artifact; interactive/debug calls use `input`. For interactive calls the
required `argv` may contain the declared target; the LaunchSpec owns actual argv.
Changing configuration requires host preparation again, not editing original
attachments. Hash failures mean an identity changed and must be investigated.

The worker uses patched execution copies for a supplied loader, in a sealed
runtime cache. Their hash can differ from the original binary. Keep original,
prepared and mapped identities distinct. Check `debug_profile` or
`interactive_profile` for actual loaded libc/loader; a prepared manifest is never
proof of a matching process. Writable cwd files are private copies per execution.

PTY is supported by interactive sessions. Automatic replay/verification currently
supports stdin pipes and argv files; it rejects PTY LaunchSpecs. GDB stdin crash
snapshots use a regular input file; record that delivery difference. A debugger
snapshot alone does not establish native replay equivalence.

## Service experiments

For `local_service`, connect from that challenge's tool worker to
`challenge:<configured port>`. `localhost` is the tool container itself. The
challenge must listen on a container-accessible interface. For `remote`, the
same alias is a fixed-destination relay; the tool stays on the internal network.
Do not infer a port or contact unrelated endpoints. Use `py_run`/pwntools to save
exact protocol exchanges and the requested success predicate.

Network access provides I/O, not ptrace or filesystem equivalence. First verify
local hypotheses against prepared files, then replay the final protocol against
the original challenge image. Report container image ID and runtime differences.
A complete jailed filesystem can be exported with `challenge export-rootfs`;
redpwn-style `/srv` and special kernel/jail settings require explicit inspection.
Host-managed `challenge extract --path ... --role binary|libc|loader` reads files
without running the image entrypoint. Never assume tool-system libc matches it.

## Symbols and portability

`challenge pwninit` operates on disposable copies with network access and a time
limit. It may unstrip a copied libc: use its output as symbol material and verify
Build-ID, do not silently replace the supplied runtime libc. All originals retain
their import hashes. Offline bundles carry image IDs, files and volume snapshots;
credentials are configured on the new host. Imported sessions are lost, not
resumed, and archived analysis history is read-only.
