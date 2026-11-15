"""Job admission reads inputs and normalizes a private spec before reservation.

No job directory, isolation UID or process is created here. The fingerprint is
computed before reservation adds an execution UID, excluding only runner_token.
"""

from dataclasses import dataclass
import hashlib
import json
from pathlib import Path
import secrets
from typing import Any
from worker_common import MAX_FILE, bounded_int, confined, public_file, safe_env


@dataclass(frozen=True)
class JobPlan:
    spec: dict[str, Any]
    fingerprint: str


def prepare_job(
    root: Path, params: dict[str, Any], kind: str, *, trusted: bool, isolated: bool
) -> JobPlan:
    if not trusted and not isolated:
        raise ValueError(
            "production execution requires a root Linux worker with UID/GID/CHOWN capabilities; use explicit trusted_local only for trusted fixtures"
        )
    spec = dict(params)
    if kind == "minimize" and any(spec.get(key) is not None for key in ("trials", "expected")):
        raise ValueError(
            "input.minimize owns candidate trials and crash equivalence; trials/expected are not minimization options"
        )
    if spec.get("network") is not None:
        _admit_network(root, spec, kind, trusted)
    elif spec.get("launch_spec"):
        _admit_launch(root, spec, kind, params.get("timeout_secs"))
    spec["isolated"] = isolated
    spec["call_id"] = spec.get("call_id") or spec.get("request_id")
    spec["kind"] = kind
    spec["timeout_secs"] = bounded_int(spec.get("timeout_secs"), 30, 1, 86400)
    spec["output_limit"] = bounded_int(spec.get("output_limit"), 1048576, 1024, 8 * 1048576)
    spec["runner_token"] = secrets.token_hex(32)
    if kind in {"replay", "verify", "minimize"}:
        _admit_replay(root, spec, kind)
    else:
        _admit_process(root, spec, kind)
    safe_env(spec.get("env"))
    # Match the complete request, not just an id, to avoid silently reusing
    # unrelated work. Reconnect callers can safely retry job creation.
    fingerprint_spec = {k: v for k, v in spec.items() if k != "runner_token"}
    fingerprint = hashlib.sha256(json.dumps(fingerprint_spec, sort_keys=True).encode()).hexdigest()
    return JobPlan(spec, fingerprint)


def _admit_network(root: Path, spec: dict[str, Any], kind: str, trusted: bool) -> None:
    from worker_network import network_profile, validate_plan

    if kind != "verify" or any(
        spec.get(key) is not None for key in ["args", "env", "expected", "input_mode"]
    ):
        raise ValueError(
            "network plans are exclusive to poc.verify and cannot override local replay parameters"
        )
    spec["trusted_local"] = trusted
    binding = network_profile(root, spec, trusted)
    if binding != spec.get("network_binding"):
        raise ValueError("network binding must match the admitted service profile")
    input_path = public_file(root, spec["input"])
    if input_path.stat().st_size > 1048576:
        raise ValueError("network input exceeds 1 MiB")
    validate_plan(spec["network"], input_path.read_bytes())


def _admit_launch(
    root: Path, spec: dict[str, Any], kind: str, requested_timeout: int | None
) -> None:
    if kind == "process" and (
        spec.get("argv") or spec.get("env") or spec.get("cwd") not in (None, ".")
    ):
        raise ValueError("LaunchSpec owns process argv, environment and cwd")
    from worker_launch import load

    launch, _, _, _, _ = load(root, spec)
    if (
        kind == "process"
        and requested_timeout is not None
        and requested_timeout != launch.get("timeout_secs", 30)
    ):
        raise ValueError("LaunchSpec owns its execution timeout")
    spec["input_mode"] = "stdin" if launch["input_mode"] == "stdin_pipe" else "argv"
    if launch["input_mode"] == "pty":
        raise ValueError(
            "PTY replay is not supported; use interact.start and verify the delivery mode explicitly"
        )
    spec.setdefault("timeout_secs", launch.get("timeout_secs", 30))


def _admit_replay(root: Path, spec: dict[str, Any], kind: str) -> None:
    target = public_file(root, spec["target"])
    input_path = public_file(root, spec["input"])
    if not target.is_file() or not input_path.is_file() or input_path.stat().st_size > MAX_FILE:
        raise ValueError("target/input must be files and input must be <=32 MiB")
    spec["timeout_secs"] = min(spec["timeout_secs"], 60)
    spec["trials"] = bounded_int(spec.get("trials"), 3 if kind == "verify" else 1, 1, 16)
    spec["max_attempts"] = bounded_int(spec.get("max_attempts"), 128, 1, 4096)
    spec["minimize_timeout_secs"] = bounded_int(spec.get("minimize_timeout_secs"), 300, 1, 3600)
    if kind == "verify":
        if (
            not spec.get("verifier_agent_id")
            or not spec.get("producer_agent_id")
            or not spec.get("call_id")
        ):
            raise ValueError("poc.verify requires verifier_agent_id, producer_agent_id and call_id")
        if spec["verifier_agent_id"] == spec["producer_agent_id"]:
            raise ValueError("independent verifier must differ from PoC producer")


def _admit_process(root: Path, spec: dict[str, Any], kind: str) -> None:
    if spec.get("launch_spec"):
        if kind != "process":
            raise ValueError("LaunchSpec owns process argv, environment and cwd")
    elif not isinstance(spec.get("argv"), list) or not spec["argv"]:
        raise ValueError("argv is required")
    cwd = confined(root, spec.get("cwd", "."), True)
    if not cwd.is_dir():
        raise ValueError("cwd must be a worker directory")
    if spec.get("input") is not None:
        source = public_file(root, spec["input"])
        if not source.is_file() or source.stat().st_size > MAX_FILE:
            raise ValueError("process input must be a staged file <=32 MiB")
