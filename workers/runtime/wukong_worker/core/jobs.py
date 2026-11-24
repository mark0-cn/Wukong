"""Prepared, fingerprinted job intent; no storage or process ownership."""

from dataclasses import dataclass
from typing import Any
from wukong_worker.core.lifecycle import JobState


@dataclass(frozen=True)
class JobPlan:
    spec: dict[str, Any]
    fingerprint: str


@dataclass(frozen=True)
class JobOutcome:
    state: JobState
    result: dict[str, Any]
    cleanup_pending: bool = False

    def __post_init__(self):
        object.__setattr__(self, "state", JobState(self.state))
