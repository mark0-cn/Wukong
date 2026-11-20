"""Read-only durable resource inventory shared by shutdown and maintenance.

Enumeration observes records only: it never deletes durable state, signals an
owned process group or publishes a cleanup decision. Completion judgement uses
the shared lifecycle observations plus the process journal's last durable
observation; cleanup remains with each resource owner.
"""

from dataclasses import dataclass
from pathlib import Path

from wukong_worker.artifacts.metadata import load_json
from wukong_worker.core.lifecycle import ResourceObservation

JOB_DIRECTORY = "jobs"
JOB_RECORD = "status.json"
SESSION_DIRECTORIES = ("debug", "interact")
SESSION_RECORD = "session.json"
PROCESS_RECORD = "processes.json"


def session_directories(directory: Path) -> list[Path]:
    """Session directories with a receipt or an interrupted publication journal.

    A crash between process ownership publication and session publication leaves
    only ``processes.json``. That interrupted session still owns resources, so
    both restart reconciliation and shutdown admission must discover it.
    """
    found = {path.parent for path in directory.glob("*/" + SESSION_RECORD)}
    found.update(path.parent for path in directory.glob("*/" + PROCESS_RECORD))
    return sorted(found)


def session_record(directory: Path) -> dict:
    """Read a session receipt; an unpublished session keeps its recovery state."""
    path = directory / SESSION_RECORD
    return load_json(path) if path.exists() else {"state": "recovery_required"}


def cleanup_observed(directory: Path) -> bool:
    """Read the journal's durable observation: only ``closed`` confirms cleanup."""
    path = directory / PROCESS_RECORD
    return not path.exists() or load_json(path).get("observed") == "closed"


@dataclass(frozen=True)
class ResourceRecord:
    """One durable resource record observed without mutation."""

    kind: str
    identifier: str
    state: str
    settled: bool

    def as_dict(self) -> dict[str, str]:
        return {"kind": self.kind, "id": self.identifier, "state": self.state}


def records(root: Path) -> list[ResourceRecord]:
    """Enumerate every durable resource record without modifying it."""
    found = []
    for path in sorted((root / JOB_DIRECTORY).glob("*/" + JOB_RECORD)):
        status = load_json(path)
        found.append(
            ResourceRecord(
                "job",
                path.parent.name,
                status["state"],
                ResourceObservation.job(status).settled,
            )
        )
    for kind in SESSION_DIRECTORIES:
        for directory in session_directories(root / kind):
            state = session_record(directory)
            observation = ResourceObservation.session(state)
            found.append(
                ResourceRecord(
                    kind,
                    directory.name,
                    state.get("state", "recovery_required"),
                    observation.settled and cleanup_observed(directory),
                )
            )
    return found


def pending_resources(root: Path) -> list[dict[str, str]]:
    """Unsettled resources in the existing maintenance display shape."""
    return [record.as_dict() for record in records(root) if not record.settled]
