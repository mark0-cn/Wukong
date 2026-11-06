# Fuzz engineering

Establish input format and launch a bounded AFL++ QEMU task against the exact staged target. Document argv/stdin/file input, seeds, memory/time limits and environment. Use job handles; avoid occupying a model call while fuzzing.

Monitor coverage/stability and crashes, retain corpus and logs, and hand artifacts to crash triage. A generated crash is unclassified until independently reproduced. Record dynamic coverage separately from static function analysis.
