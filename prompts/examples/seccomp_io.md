# Worked example: a pre-opened descriptor under a filter

A target opens a data file before installing a filter. The task is to recover
the already available data, while opening a new file is rejected. Record the
open result, descriptor lifetime, filter-install point and the actual policy
artifact. Do not hardcode the first available descriptor.

Use seccomp_analyze to compare concrete read/write/open probes with the correct
AUDIT_ARCH, syscall ABI and arguments. Then replay the file-read behavior
natively. Preserve returned byte counts and raw output. A successful analyzer
probe alone does not establish that the same filter stack was installed.

The original fixture in fixtures/pwn/src/seccomp_fd.c has an existing-FD path
and a denied-open control. These observations support constrained file I/O in
that fixture. They do not support execve, a shell, arbitrary filesystem access,
or applicability to another kernel/jail configuration. Network verification
must additionally preserve its configured service binding and transcripts.
