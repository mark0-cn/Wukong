# Worker toolchain quick reference

Use `runtime_doctor` to check the selected worker's actual capabilities.
The bundled worker includes Python 3, pwntools, z3-solver, angr and ropper;
other configured workers may not. Reach available tools through execution.

## exec_command — the default for quick work

`exec_command` runs ONE bounded shell command synchronously and returns
stdout/stderr/exit code in the result. Prefer it for inspection, file
surgery and stdin-driven PoCs:

- The command runs in a FRESH temporary directory — nothing from the state
  is present unless you make it present. Pass `target=<staged path from
  stage_target>` and the worker places the resolved target at `$TARGET`
  in the environment; the worker also stages it automatically in isolated
  jobs. Use `"$TARGET"` in the command, e.g.
  `printf '<payload>' | "$TARGET"` or `objdump -d "$TARGET"`.
- Worker paths like `files/...` resolve only as DIRECT argv entries; a path
  written inside the command string is not resolved. Never `find /` or
  `ls /state` to locate the target — it is either at `$TARGET` (pass
  `target=`) or staged via argv in job_start. If you catch yourself
  searching for a file, you are missing one of those two mechanisms.
- stdin delivery is plain shell syntax: `printf '<payload>' | "$TARGET"`,
  a heredoc, or `python3 -c '...'`. There is no separate input plumbing.
- The call is capped (timeout_secs, default 30; output is bounded). Anything
  longer goes to the background with its output redirected:
  `cmd > /tmp/out.log 2>&1 &` — then read /tmp/out.log with a later
  exec_command or file_get. Do not block exec_command on long work.
- Multi-round interaction belongs to interactive_start/send, not exec_command.

## py_run — scripts in one step

For any script longer than one line, prefer `py_run` over
file_put+job_start: pass the source text directly (no base64 encoding),
and the worker syntax-checks it inline before starting the job — a syntax
error comes back immediately instead of wasting a job round-trip.
Optional `argv` entries arrive as `sys.argv[1..]`; an optional
`target=<staged path>` is appended LAST and staged automatically — read it
from `sys.argv[-1]`. Read stdout/stderr via job_status.

## emulate_run — bounded Unicorn emulation

For VM-style or hand-crafted code (interpreter loops, x87/FPU blocks,
self-decoding payloads), prefer `emulate_run` over hand-building a
Unicorn harness in py_run: it maps the ELF segments plus a stack, runs
from `start` for up to `count` instructions or until the PC reaches
`until`, and returns registers plus requested `dump` ranges. Hand-built
harnesses historically burned turns on hook-page and endianness bugs —
this tool is those bugs pre-fixed. It is a pure simulation: the target
never runs natively. Use `init_regs` for register preconditions and
`memory` to inject bytes at addresses.

## one_gadget and libc offsets

- `one_gadget` (ruby) is in the worker: `one_gadget -l 1 /lib/x86_64-linux-gnu/libc.so.6`
  lists single-shot execve gadgets with their constraints.
- Symbol offsets from the worker's own libc: `python3 -c "from pwn import ELF; print(hex(ELF('/lib/x86_64-linux-gnu/libc.so.6').symbols['system']))"`.
  Compare against the leaked address to compute the libc base in one step.
- `libc_identify` does the same offline in one call: pass the leaked
  runtime address and the leaked symbol name; it returns the libc base,
  system_offset and a /bin/sh offset when the worker's own libc matches.
  A no-match means the target runs a different libc than the worker.

## patchelf and gef

- `patchelf --set-interpreter /lib64/ld-linux-x86-64.so.2 --replace-needed
  libc.so.6 <worker-libc> ./target` re-links an old binary against the
  worker's libc for local runs (copy the target first; never patch the
  staged original).
- `gef` is available at /opt/gef.py for interactive gdb work: run
  `gdb -q ./target -ex "source /opt/gef.py"` and use `heap chunks`,
  `heap bins`, `tcache bins` for allocator debugging. The debug_start
  session itself stays plain GDB/MI.

## job_start — persistent background jobs

For long-running or multi-step work, write a script with file_put and run it
with job_start (argv ["python3", "files/<script>"]), then read job_status.
Before job_start, syntax-check the script once:
`exec_command` `python3 -m py_compile files/<script>` (or run the script with
`--help`-style harmless flags) — a one-character syntax error discovered
inside a job wastes the whole job round-trip, and at quota's edge it can
cost the rest of your budget.
Prefer `python3 files/<script>` over executing the script directly:
file_put stages files without the executable bit unless executable=true, and
a direct exec of a non-executable file fails with Permission denied.

Terminal job results arrive automatically in the next request's
`job_results` section — do not poll `job_status` unless a result is missing.

## Path discipline (worker layouts are not guessable)

- Worker paths like `files/...` resolve only as DIRECT argv entries; a path
  written inside a script body or a `bash -c` string is not resolved.
- Pass the staged target into your script as an argv element with the
  `{target}` placeholder: `job_start argv ["python3", "files/<script>", "{target}"]`;
  the harness replaces it with the staged target path and the script reads it
  from `sys.argv[2]`. In isolated jobs that argv entry is staged automatically.
- Pass every additional input file or directory as a direct argv entry too;
  staging resolves it to the experiment's read-only copy. Read the resulting
  `sys.argv` values inside the script. `RE_AGENT_STATE_DIR` and
  `RE_AGENT_JOB_DIR` identify private worker metadata in isolated mode, not
  accessible input/output directories. Write outputs under the process's
  current working directory and collect them with `job_artifacts`.
- Match the input channel: `job_start` itself has no stdin payload field.
  For stdin input, stage a wrapper and input file as argv, and let the wrapper
  feed the file bytes to the target's stdin — or just use exec_command with a
  pipe. `crash_replay`/`poc_verify` deliver the input file through the `@@`
  argv placeholder by default; set `input_mode: "stdin"` to pipe the bytes
  to the target's stdin instead. Use `interactive_start/send` for multi-round
  input.

## angr (symbolic execution) — fastest path for input reconstruction

The current `angr_solve` backend creates a bounded symbolic stdin stream.
Use it when that matches the target's input channel and required operations
can be modeled. It does not model arbitrary argv/file/network inputs. Use
explicit z3 constraints or an appropriate harness for those cases:

- stage_target first; pass the staged path, the module-relative `find`
  address (the success path, e.g. the address after the final check), and
  optional `avoid` addresses (failure prints, abort) to `angr_solve`.
- It returns a job_id; poll job_status. On completion stdout carries
  {"solved": true, "input_base64": ..., "input_hex": ..., "stats": {...}}.
  The solved input is also persisted as job artifacts (solved-input.bin,
  solved.json) and imported as evidence when the terminal job_status is
  polled — prefer those artifacts over scraping the stdout tail.
  Decode input_base64 and feed it to the target with job_start to CONFIRM
  the solution really reaches the find address — angr's answer is a
  hypothesis until the real run agrees.
- Cap the search: stdin_length near the expected input size (64 default),
  max_steps and timeout_secs (default 600). If it dead-ends, loosen
  printable_only=false or add more find addresses.
- Example solved state JSON: {"solved":true,"input_base64":"ZmxhZ3t...","stats":{"found":1,"deadended":0}}

## z3 for custom constraints

When angr cannot model the logic (template metaprograms, checked math),
solve it yourself:

```python
from z3 import *
x = BitVecs(' '.join(f'x{i}' for i in range(16)), 8)
s = Solver()
s.add(x[0] == 0x67 ^ 1)          # your recovered constraints
s.add(x[1] + 3 == 0x6a)
print(s.check(), s.model())
```

Run the script, take the concrete bytes, and verify them against the real
target before believing them.

## pwntools for pwn plumbing

- service interaction: `from pwn import *; io = remote('127.0.0.1', PORT)`
  (or process for local), `io.recvuntil(b'> ')`, `io.sendline(payload)`,
  `print(io.recvall(timeout=2))`.
- offset finding: `cyclic(200)` to generate, `cyclic_find(b'kaaa')` on a
  crash to recover the offset.
- `ELF('./target')` gives symbols/plt/got; `ROP(elf)` builds chains.
- Keep every script printing repr() of received bytes; raw control bytes
  belong in the payload, not in the transcript.

## ropper for gadget hunting

`job_start` argv ["ropper","--file",<staged target>,"--search","pop rdi"]
(or "ret", "syscall"). Output is bounded by the job; search one gadget
class per call.

## Discipline

- Timeout every job; angr and z3 can run away. timeout_secs is your friend.
- The solver's answer is evidence only after the real binary reproduces it:
  run the solved input and cite both the solver job and the replay job.

## Reproduce the exact runtime

Use runtime_profile on staged target/libc/loader files. File identity and page-aligned
symbol subtraction are candidate evidence, not proof of a loaded library. An owned live
debugger session enables observed mappings and per-file mapped_hash_confirmed status.
For supplied runtime files, runtime_prepare returns immutable copies and direct launch_argv;
pass those argv entries unchanged to job_start or interactive_start. Extra libraries may
still be missing. Preserve the original target and all runtime hashes. Never patch the
original binary or silently substitute the worker's libc. A mapping that could not be
hashed or a missing process remains unknown, not a confirmed match.

### Crash observations

`crash_snapshot` accepts exactly one owned stopped `session_id` or staged `core`.
It returns bounded registers, memory, mappings and frame observations; unknown
module identity remains unknown. `cyclic_offset` requires the original `pattern`,
`alphabet`, `n`, and exactly n `observed_base64` bytes. `input_minimize` requires
GDB and matching location signatures by default; the explicit `allow_signal_only`
fallback labels weaker results. Both replay and minimization support `input_mode`.

### Protection observations

`mitigation_profile` separates static target declarations, worker environment
support and owned process observations. Use `session_id` or
`interactive_session_id`, never a raw PID. Optional `address` returns the exact
containing mapping's permissions. SHSTK status is per-thread; IBT and seccomp
policy remain unknown unless separate evidence establishes them. See
`mitigation_analysis` when those protections determine the next experiment.
