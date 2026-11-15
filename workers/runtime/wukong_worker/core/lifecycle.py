"""Pure v3 completion rules shared by resource services and maintenance probes."""

from dataclasses import dataclass
from enum import StrEnum
from typing import Any


class JobState(StrEnum):
    QUEUED = "queued"
    RUNNING = "running"
    COMPLETED = "completed"
    FAILED = "failed"
    CANCELLED = "cancelled"
    TIMED_OUT = "timed_out"
    LOST = "lost"


TERMINAL = frozenset(JobState) - {JobState.QUEUED, JobState.RUNNING}


class CleanupState(StrEnum):
    PENDING = "pending"
    UNCONFIRMED = "unconfirmed"
    CONFIRMED = "confirmed"


class OutcomeCertainty(StrEnum):
    KNOWN = "known"
    UNKNOWN = "unknown"


def optional_bool(record, name):
    value = record.get(name)
    if value is not None and type(value) is not bool:
        raise ValueError(f"{name} must be a boolean")
    return value


@dataclass(frozen=True)
class ResourceObservation:
    terminal: bool
    outcome: OutcomeCertainty
    cleanup: CleanupState

    @property
    def settled(self) -> bool:
        return self.terminal and self.cleanup == CleanupState.CONFIRMED

    @classmethod
    def job(cls, record: dict[str, Any]) -> "ResourceObservation":
        state = JobState(record["state"])
        terminal = state in TERMINAL
        pending = optional_bool(record, "cleanup_pending")
        result = record.get("result") or {}
        outcome = OutcomeCertainty(
            result.get("outcome") or ("unknown" if state == JobState.LOST else "known")
        )
        cleanup = (
            CleanupState.PENDING
            if pending
            else (CleanupState.CONFIRMED if terminal else CleanupState.UNCONFIRMED)
        )
        return cls(terminal, outcome, cleanup)

    @classmethod
    def session(cls, record: dict[str, Any]) -> "ResourceObservation":
        state = record.get("state", "recovery_required")
        if state not in {"running", "exited", "closing", "recovery_required", "closed", "lost"}:
            raise ValueError("invalid session state")
        pending = optional_bool(record, "cleanup_pending")
        confirmed = optional_bool(record, "cleanup_confirmed")
        proof = record.get("result") or {}
        native = all(
            optional_bool(proof, key) is True
            for key in ("jobs_empty", "profiles_removed", "scratch_removed")
        )
        closed = state == "closed" or (
            state == "lost" and (confirmed is True or (pending is False and native))
        )
        cleanup = (
            CleanupState.PENDING
            if pending
            else (CleanupState.CONFIRMED if closed else CleanupState.UNCONFIRMED)
        )
        outcome = (
            OutcomeCertainty.UNKNOWN
            if state in {"lost", "recovery_required"}
            else OutcomeCertainty.KNOWN
        )
        return cls(state in {"closed", "lost"}, outcome, cleanup)
