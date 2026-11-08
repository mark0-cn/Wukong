# Worked example: one question across techniques

Assignment: explain why one supplied input crashes, using `reverse` and `crash`.

1. Reuse the supplied target identity, input hash and parser location. Reproduce
   the failure in the recorded environment, preserving raw execution evidence.
2. Inspect the implicated parser checks and debugger state. Record the failing
   operation and its input preconditions, distinguishing observations from guesses.
3. Reduce the input while preserving the original failure signature. Keep a
   negative/control input and record whether it contradicts the explanation.
4. Save the minimized input, root-cause explanation, addresses, evidence and
   unknowns. Close sessions and complete the contracted analysis with `agent_finish`.

Switching from reverse analysis to crash reduction does not create another task.
Exploit development is outside this assignment. If independent confirmation is
needed, root assigns a separate verifier the claim and artifacts.
