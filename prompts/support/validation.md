# Testable script validation (self_check_v1)

This method grants no permissions. Producers create the script; independent
verifiers inspect collector/evaluator wiring and execute acceptance. Use only
the assigned worker. Never execute a target or import its script on the controller.

The producer contract declares `artifact_acceptance: {"solve_script":"self_check_v1"}`.
Keep the script standalone. Before sending a large script, syntax-check a small
version with py_run; save it with file_put path_policy=versioned, then use
file_revise for uniquely matching small changes. Reuse returned worker paths.

Separate observation collection from verdict evaluation. Every promised check
must appear in the manifest. A process check declares id, kind="process",
expected_exit_code (nonnegative integer), required_stdout and forbidden_stdout
(lists of nonempty strings). A comparison check declares id, kind="comparison"
and expected (JSON value). All ids are unique. Keep at most 32 checks and 8
required/forbidden strings per process. Do not generalize exit!=0 to rejection:
the wrong-input target may intentionally exit 0 or another declared code.

The real --verify collects a baseline with schema_version=1, manifest (list)
and observations (object keyed by check id). Process observations contain state
(completed/timed_out/...), exit_code, signal (null on normal termination),
stdout and output_complete. Mark capture truncation or unavailable output
incomplete; never invent an exit code. Comparison observations contain observed.
Write baseline.json in the current writable directory so job_artifacts imports it.
Exit 0 means pass, 1 means a contradicted expectation, 2 means inconclusive.

Add a --validation-io mode accepting one JSON request on stdin. `describe`
returns exactly {"schema_version":1,"manifest":[...]}, exit 0. `evaluate`
receives schema_version=1, operation="evaluate", observations={...}; use the
SAME evaluator as real --verify, output {"status":"pass|fail|inconclusive"}
and exit 0/1/2. This mode MUST NOT launch the target. Handle it before ordinary
CLI parsing. Describe uses the same manifest as the real run, never a separate
test-only predicate. Preserve original target arguments after --validation-io.

Standalone Python evaluator example (replace only the manifest and collector;
do not weaken these predicates):

```python
def validation_status(manifest, observations):
    failed, missing = False, False
    for check in manifest:
        item = observations.get(check['id'])
        if not isinstance(item, dict):
            missing = True
            continue
        if check['kind'] == 'comparison':
            missing |= 'observed' not in item
            failed |= ('observed' in item and json.dumps(item['observed'],sort_keys=True,allow_nan=False)
                       != json.dumps(check['expected'],sort_keys=True,allow_nan=False))
        elif check['kind'] == 'process':
            complete = (item.get('state') == 'completed'
                        and item.get('output_complete') is True
                        and type(item.get('exit_code')) is int
                        and 'signal' in item and isinstance(item.get('stdout'), str))
            if not complete:
                missing = True
                continue
            failed |= (item['exit_code'] != check['expected_exit_code']
                       or item['signal'] is not None
                       or any(s not in item['stdout'] for s in check['required_stdout'])
                       or any(s in item['stdout'] for s in check['forbidden_stdout']))
        else:
            missing = True
    return 'inconclusive' if missing else 'fail' if failed else 'pass'
```

Python dispatch: json.load(sys.stdin), validate schema_version and operation;
describe prints the manifest, evaluate calls validation_status then prints
status and sys.exit({'pass':0,'fail':1,'inconclusive':2}[status]). A Shell script
may embed this Python code with `python3 -c '...' "$@"` and keep stdin for the
JSON request. Declare its Python 3 dependency. Do not use a heredoc that consumes
the request's stdin. Both languages use the same external protocol.

Verifier workflow: stage the exact final script and target, run the documented
--verify command with explicit executable_inputs for any nested executable.
Import baseline.json and raw status. Independently inspect the original script
to establish that real and injected observations share the same collector-to-
evaluator path. Record analysis_verify kind=validator_integration with the exact
artifact_evidence_id, reproduction_job_id, expected_exit_code=0, owned code-review
evidence and runtime status. Then artifact_validate with that artifact, baseline
evidence/job, interpreter=python3 or bash, and original arguments. Wait for its
job result. Missing protocol, failed cases or unsupported environments are gaps,
not PASS. Revisions require acceptance of the new hash.

The driver simulates altered observations: nonzero/signal after success output,
timeout, missing checks/exit status, truncated output and contradictory values.
It does not prove that the target really exhibited these faults, that the
manifest covers every claim, or that arbitrary source semantics are correct.
Keep independent real-target positive/negative controls and explicit limitations.
