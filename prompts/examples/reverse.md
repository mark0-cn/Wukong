# Worked example: reconstruct a numeric transformation

Task: recover the fixture's numeric routine, not a vulnerability. The input
channel (one decimal argv value) and `transform_word` location are already in
recon evidence.

1. Load `algorithms`, decompile the returned function offset and inspect its
   relevant references. Recover a candidate multiply/add, 32-bit wrap,
   rotate-left by 7 and final XOR. The constants are fixture-specific facts,
   not values to assume for a new target.
2. Write an independent checker such as the following, upload it with
   `file_put`, then `job_start` with argv `["python3", <returned script path>,
   "{target}"]`. The script receives the staged target in sys.argv[1].

```python
import json, subprocess, sys
for x in [0, 1, 0x7fffffff, 0x80000000, 0xffffffff]:
    mixed = (x * 1664525 + 1013904223) & 0xffffffff
    expected = (((mixed << 7) | (mixed >> 25)) & 0xffffffff) ^ 0xa5a5a5a5
    p = subprocess.run([sys.argv[1], str(x)], capture_output=True, timeout=2)
    observed = p.stdout.decode().strip()
    print(json.dumps({"x": x, "expected": f"{expected:08x}",
                      "observed": observed, "exit_code": p.returncode}))
    assert p.returncode == 0 and observed == f"{expected:08x}"
```

3. Read job_status and import artifacts. In this fixture, 0 -> 92dc0a3b,
   1 -> e18913bb and 4294967295 -> 8f630cbb. Preserve disagreements instead
   of editing expected values to fit the output.
4. Save pseudocode, integer widths, decimal/hex I/O conventions and the
   expected/observed vector table with static and execution evidence IDs.
   Finish with the bounded conclusion: the recovered formula is consistent
   with the inspected routine and tested vectors; untested environments and
   inputs remain outside the observed execution coverage.
