"""Worker shutdown use case; the host retains the final stopped state.

The host validates the request and concurrency admission before delegating. This
use case preserves the established order: reject active jobs, close the sessions
this daemon owns, then re-check durable resources and supervised process exit.
Maintenance and shutdown share the same inventory judgement but publish their
own results.
"""

from wukong_worker.core.lifecycle import TERMINAL
from wukong_worker.resources.inventory import pending_resources


def shutdown_worker(root, jobs, sessions) -> None:
    """Confirm that no owned resource remains, or raise the existing refusal.

    A failed or unconfirmed cleanup leaves its durable record and blocking state
    untouched; callers retry the same admission until every owned resource is
    settled. Only the caller may mark the host stopped.
    """
    active = [jobs.status(path.parent.name) for path in (root / "jobs").glob("*/status.json")]
    if any(job["state"] not in TERMINAL for job in active):
        raise ValueError(
            "cannot shut down worker while jobs are active; cancel and reconcile them first"
        )
    sessions.close_all()
    if pending_resources(root):
        raise ValueError("cannot shut down worker while resource cleanup remains unconfirmed")
    jobs.process_resources.wait()
