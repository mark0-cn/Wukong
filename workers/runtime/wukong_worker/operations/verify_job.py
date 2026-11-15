"""Owned job execution policy; returns observations without publishing job state."""

import platform
from wukong_worker.artifacts.common import artifact, confined
from wukong_worker.core.jobs import JobOutcome

from wukong_worker.operations.replay import replay, expectation_matches


def execute(root, directory, spec):
    input_bytes = confined(root, spec["input"], True).read_bytes()
    results = []
    for index in range(spec.get("trials", 1)):
        if (directory / "cancel").exists():
            break
        if spec.get("network") is not None:
            from wukong_worker.network.service import replay_network

            results.append(replay_network(root, directory, spec, input_bytes, index))
        else:
            results.append(replay(root, directory, spec, input_bytes, index))
    successes = sum(
        r["network_success"]
        if spec.get("network") is not None
        else expectation_matches(r, spec.get("expected"))
        for r in results
    )
    result = {
        "trials": len(results),
        "attempts": len(results),
        "requested_trials": spec.get("trials", 1),
        "successes": successes,
        "passed": bool(results)
        and len(results) == spec.get("trials", 1)
        and successes == len(results),
        "observed_stage": "network_behavior"
        if spec.get("network") is not None
        else "crash"
        if results and all(r["crash_signature"] for r in results)
        else "behavior",
        "results": results,
        "verifier_agent_id": spec.get("verifier_agent_id"),
        "producer_agent_id": spec.get("producer_agent_id"),
        "call_id": spec.get("call_id"),
        "target": artifact(root, confined(root, spec["target"], True)),
        "input": artifact(root, confined(root, spec["input"], True)),
        "expected": spec.get("expected"),
        "environment": {
            "platform": platform.platform(),
            "python": platform.python_version(),
        },
        "fresh_working_directory_per_trial": True,
    }
    if spec.get("network") is not None:
        result.update(
            network_binding=spec["network_binding"],
            expected=spec["network"],
            fresh_connection_per_trial=all(r["fresh_connection"] for r in results),
            server_reset_per_trial=False,
        )
    state = "cancelled" if (directory / "cancel").exists() else "completed"
    return JobOutcome(state, result)
