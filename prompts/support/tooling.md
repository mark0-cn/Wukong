# Worker toolchain quick reference

Use `runtime_doctor` to check the selected worker's actual capabilities.
The bundled worker includes Python 3, pwntools, z3-solver, angr and ropper;
other configured workers may not. Reach available tools through execution.

## exec_command — the default for quick work

`exec_command` runs ONE bounded shell command synchronously and returns
stdout/stderr/exit code in the result. Prefer it for inspection, file
surgery and stdin-driven PoCs:

- stdin delivery is plain shell syntax: `printf '<payload>' | {staged-target-path}`,
  a heredoc, or `python3 -c '...'`. There is no separate input plumbing.
- stage_target first and use the returned path (or pass `{target}` into the
  command string) for the binary itself.
- The call is capped (timeout_secs, default 30; output is bounded). Anything
  longer goes to the background with its output redirected:
  `cmd > /tmp/out.log 2>&1 &` — then read /tmp/out.log with a later
  exec_command or file_get. Do not block exec_command on long work.
- Multi-round interaction belongs to interactive_start/send, not exec_command.

## job_start — persistent background jobs

For long-running or multi-step work, write a script with file_put and run it
with job_start (argv ["python3", "files/<script>"]), then read job_status.

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
