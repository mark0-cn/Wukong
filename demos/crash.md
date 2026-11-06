# Worked example: crash triage

Reproduce first (`crash_replay` with the recorded input and target), then `input_minimize` keeping the original failure signature. Group by normalized root cause, not by ASLR addresses. A crash is not exploitability.
