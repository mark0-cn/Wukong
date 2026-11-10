# Reachable command-execution sinks

Use when the goal may be served WITHOUT memory corruption: the binary itself
imports an exec-family symbol (system, popen, execve, posix_spawn) and some
caller may pass an input-tainted argument to it. A run/shell/execute feature
inside the program is a candidate sink, not noise.

## Find the sink first, then the input path

1. Enumerate every caller of the exec-family imports. For each caller, check
   whether any argument derives from input (command tokens, buffer contents,
   environment-derived strings). A fixed-argument call is not a sink.
2. Pick the sink whose tainted argument needs the SHORTEST input path — the
   closest input to the call, not the first one found.
3. Verify reachability empirically: run the feature with a benign marker
   command and observe its effect (e.g. a command whose output differs from
   normal program output). The observation, not the reasoning, proves the sink.
4. Deliver the minimal trigger: input that reaches the sink and makes the
   intended command execute. Include the caller, the tainted argument position
   and the observed effect transcript as evidence.

## Disambiguation

- A decompiler showing `system(argv[i])` is a hypothesis; the live run with a
  marker command is the verification.
- Shell interpretation happens inside the sink: quoting, metacharacters and
  the actual shell differ from the caller's parser. Test the delivered bytes,
  not the intended meaning.
- If the sink needs privileges or state the input cannot reach, say so instead
  of widening the search to speculative memory bugs.

Stop once the trigger reaches the sink and the observed effect matches; hand
the transcript and the minimal trigger to a verifier when the contract asks
for impact.
