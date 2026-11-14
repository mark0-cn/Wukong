"""Picire owns reduction; the caller owns execution and crash equivalence."""
import time

from picire import DD, LimitReduction, Outcome, ReductionStopped
from picire.cache import NoCache


def reduce_input(data, evaluate, *, max_attempts, deadline, cancelled):
    """Reduce an already verified baseline, counting only real executions.

    ``evaluate(candidate, index)`` must preserve the original crash identity and
    retain its trial evidence, returning True/False or None for an unresolved
    outcome. Baseline execution is trial zero and consumes one
    attempt. A cancelled/budgeted result is an observed best candidate, not a
    claim of minimality. No target, file path or subprocess is selected here.
    """
    best = data
    attempts = 1
    budget = LimitReduction(max_tests=max_attempts - 1)

    def stop():
        if cancelled():
            raise ReductionStopped("cancelled")
        if time.monotonic() >= deadline:
            raise ReductionStopped("deadline expired")

    def test(config, _content, _identity):
        nonlocal best, attempts
        stop()
        candidate = bytes(config)
        # Picire reasserts each accepted configuration before its next split.
        # Its original verified trial is sufficient within this reduction.
        if candidate == best:
            return Outcome.FAIL
        budget()
        index = attempts
        attempts += 1
        outcome = evaluate(candidate, index)
        if outcome is None:
            raise ReductionStopped("candidate outcome unresolved")
        if outcome:
            best = candidate
            return Outcome.FAIL
        return Outcome.PASS

    try:
        # DD stops at one unit; check empty input explicitly so a one-byte
        # result never receives a false minimality claim.
        if test(b"", None, None) is not Outcome.FAIL:
            best = bytes(DD(test, stop=stop, cache=NoCache())(data))
        stop()
        return best, {"attempts": attempts, "complete": True, "reducer": "picire", "stop_reason": None}
    except ReductionStopped as error:
        return best, {"attempts": attempts, "complete": False, "reducer": "picire", "stop_reason": str(error)}
