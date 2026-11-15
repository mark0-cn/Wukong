"""Owned job execution policy; returns observations without publishing job state."""

import time
from wukong_worker.artifacts.common import artifact, confined
from wukong_worker.core.jobs import JobOutcome

from wukong_worker.operations.replay import replay, crash_signature


def execute(root, directory, spec):
    data = confined(root, spec["input"], True).read_bytes()
    deadline = time.monotonic() + spec["minimize_timeout_secs"]
    baseline = replay(root, directory, spec, data, 0)
    signature = crash_signature(baseline)
    if not signature:
        raise ValueError(
            "input does not reproduce a crash; minimization requires a stable crash signature"
        )
    if not spec.get("allow_signal_only", False) and not baseline.get("location_signature"):
        raise ValueError(
            "location signature unavailable; inspect trial-0000/crash-snapshot.json. Install GDB/pygdbmi or explicitly opt into allow_signal_only (weak crash equivalence)."
        )
    from wukong_worker.execution.minimize import reduce_input

    def equivalent(candidate, index):
        observed = replay(root, directory, spec, candidate, index)
        if observed.get("state") != "completed":
            return None
        if (
            not spec.get("allow_signal_only", False)
            and crash_signature(observed)
            and not observed.get("location_signature")
        ):
            return None
        return crash_signature(observed) == signature

    data, reduction = reduce_input(
        data,
        equivalent,
        max_attempts=spec["max_attempts"],
        deadline=deadline,
        cancelled=lambda: (directory / "cancel").exists(),
    )
    minimized = directory / "minimized.bin"
    minimized.write_bytes(data)
    result = {
        "artifact": artifact(root, minimized),
        "original_size": confined(root, spec["input"], True).stat().st_size,
        "minimized_size": len(data),
        "crash_signature": signature,
        **reduction,
        "signature_strength": "signal_only"
        if spec.get("allow_signal_only", False)
        else "location_and_frames",
    }
    state = "cancelled" if (directory / "cancel").exists() else "completed"
    return JobOutcome(state, result)
