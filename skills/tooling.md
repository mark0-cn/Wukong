# Worker toolchain quick reference

The worker has Python 3 with pwntools, z3-solver, angr and ropper installed.
You reach them through the normal execution tools — write a script with
file_put and run it with job_start (argv ["python3", "files/<script>"]),
then read job_status. Worker paths resolve only as direct argv entries;
never wrap them in `bash -c "..."`.

## angr (symbolic execution) — fastest path for input reconstruction

When a reverse handoff says "input must satisfy these checks", prefer the
dedicated `angr_solve` tool over hand-written solving:

- stage_target first; pass the staged path, the module-relative `find`
  address (the success path, e.g. the address after the final check), and
  optional `avoid` addresses (failure prints, abort) to `angr_solve`.
- It returns a job_id; poll job_status. On completion stdout carries
  {"solved": true, "input_base64": ..., "input_hex": ..., "stats": {...}}.
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
