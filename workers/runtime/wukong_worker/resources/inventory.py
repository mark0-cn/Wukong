"""Durable resource completion inventory, shared by shutdown and maintenance."""

from wukong_worker.artifacts.metadata import load_json
from wukong_worker.core.lifecycle import ResourceObservation


def pending_resources(root, jobs):
    active = []
    for path in (root / "jobs").glob("*/status.json"):
        status = jobs.status(path.parent.name)
        if not ResourceObservation.job(status).settled:
            active.append({"kind": "job", "id": path.parent.name, "state": status["state"]})
    for kind in ["debug", "interact"]:
        records = set((root / kind).glob("*/session.json"))
        records.update(p.parent / "session.json" for p in (root / kind).glob("*/processes.json"))
        for path in sorted(records):
            state = load_json(path) if path.exists() else {}
            journal = path.parent / "processes.json"
            if not ResourceObservation.session(state).settled or (
                journal.exists() and load_json(journal)["observed"] != "closed"
            ):
                active.append(
                    {
                        "kind": kind,
                        "id": path.parent.name,
                        "state": state.get("state", "recovery_required"),
                    }
                )
    return active
