"""Owned dynamic-trace execution; returns observations without publishing job state."""

import hashlib
import json
import subprocess
import time

from wukong_worker.artifacts.common import public_file, sha256_file
from wukong_worker.artifacts.metadata import atomic_json
from wukong_worker.core.jobs import JobOutcome
from wukong_worker.debugger.gdb import DebugSession
from wukong_worker.debugger.trace import TraceDriver
from wukong_worker.execution.launch_spec import read_launch
from wukong_worker.execution.staging import execution_metadata
from wukong_worker.operations.trace_plan import logical_plan, plan_signature, validate_spec


def canonical_bytes(value):
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode(
        "utf-8"
    )


def gdb_backend():
    version = None
    try:
        version = (
            subprocess.run(["gdb", "--version"], capture_output=True, timeout=5, check=False)
            .stdout.decode(errors="replace")
            .splitlines()[0][:256]
        )
    except (OSError, subprocess.TimeoutExpired, IndexError):
        pass
    return {
        "engine": "gdb_mi",
        "gdb_version": version,
        "platform": "linux",
        "architecture": "x86_64",
    }


def failure_observation(driver, reason, error=None):
    process = driver.process or ({"error": str(error)[:1024]} if error else {})
    return {
        "bindings": driver.bindings,
        "complete": False,
        "reason": reason,
        "events_written": driver.sequence,
        "limits_hit": driver.limits_hit,
        "capture_errors": driver.capture_errors + ([str(error)[:1024]] if error else []),
        "open_call_ids": [
            activation["call_id"] for stack in driver.activations.values() for activation in stack
        ],
        "process": process,
    }


def execute(root, directory, spec):
    trace_spec = validate_spec(spec["spec"])
    launch_document = read_launch(root, spec["launch_spec"])
    input_path = public_file(root, spec["input"])
    backend = gdb_backend()
    worker_identity = spec.get("_worker_identity")
    if not isinstance(worker_identity, dict):
        raise ValueError("trace worker identity is missing")
    deadline = time.monotonic() + spec["timeout_secs"]
    trace_directory = directory / "trace"
    session = None
    driver = None
    observation = None
    state = "completed"
    stream_error = None
    cleanup_error = None
    try:
        session = DebugSession(
            root,
            spec,
            isolated=spec.get("isolated", False),
            directory=f"jobs/{directory.name}/trace/debugger",
        )
        if session.launch is None:
            raise ValueError("dynamic tracing requires a LaunchSpec")
        for point in trace_spec["points"]:
            if point["location"]["module_sha256"] != session.source_target_sha256:
                raise ValueError("trace point module does not match the LaunchSpec source target")
        driver = TraceDriver(
            session,
            trace_spec,
            trace_directory / "events.jsonl",
            directory / "cancel",
            deadline,
        )
        try:
            observation = driver.run()
            if observation["reason"] == "cancelled":
                state = "cancelled"
        except TimeoutError as error:
            state = "timed_out"
            observation = failure_observation(driver, "timeout", error)
        except Exception as error:
            state = "failed"
            observation = failure_observation(driver, "error", error)
    except Exception:
        if session is not None:
            try:
                session.close()
            except Exception:
                pass
        raise
    stream = {"jsonl_bytes": 0, "jsonl_sha256": None, "events_written": 0}
    if driver is not None:
        try:
            stream = driver.close()
        except Exception as error:
            stream_error = error
    if session is not None:
        try:
            session.close()
        except Exception as error:
            cleanup_error = error
    if observation is None:
        observation = failure_observation(driver, "error", cleanup_error or stream_error)
    if stream_error is not None:
        observation["complete"] = False
        observation["capture_errors"] = observation["capture_errors"] + [str(stream_error)[:1024]]
        if state == "completed":
            state = "failed"
    if cleanup_error is not None:
        observation["complete"] = False
        observation["capture_errors"] = observation["capture_errors"] + [str(cleanup_error)[:1024]]
    launch = session.launch
    input_identity = {
        "sha256": sha256_file(input_path),
        "size": input_path.stat().st_size,
        "mode": launch["input_mode"],
    }
    manifest = launch_document.spec
    environment = {
        "worker_identity": worker_identity,
        "backend": backend,
        "args": manifest.get("args", []),
        "cwd": manifest.get("cwd"),
        "env": manifest.get("env", {}),
        "input_mode": manifest.get("input_mode"),
        "timeout_secs": manifest.get("timeout_secs"),
        "non_target_files": {
            name: digest for name, digest in manifest["files"].items() if name != manifest["target"]
        },
    }
    completion = {
        "stream_closed": stream_error is None,
        "complete": observation["complete"] and stream_error is None,
        "reason": observation["reason"],
        "events_written": stream["events_written"],
        "jsonl_bytes": stream["jsonl_bytes"],
        "jsonl_sha256": stream["jsonl_sha256"],
        "process": observation["process"],
        "limits_hit": observation["limits_hit"],
        "capture_errors": observation["capture_errors"][:64],
        "open_call_ids": observation["open_call_ids"],
        "cleanup": {
            "session_closed": cleanup_error is None,
            "journal_closed": cleanup_error is None,
            "error": str(cleanup_error)[:1024] if cleanup_error else None,
        },
    }
    document = {
        "schema_version": 1,
        "kind": "wukong.trace.run",
        "backend": backend,
        "worker_identity": worker_identity,
        "spec": trace_spec,
        "logical_plan": logical_plan(trace_spec),
        "plan_signature": plan_signature(trace_spec),
        "launch": {
            "manifest": manifest,
            "manifest_sha256": hashlib.sha256(launch_document.raw).hexdigest(),
            "launch_identity": session.launch_identity,
            "target_member": manifest["target"],
            "source_target_sha256": session.source_target_sha256,
            "executed_target_sha256": session.executed_target_sha256,
            "input": input_identity,
            "environment_signature": hashlib.sha256(canonical_bytes(environment)).hexdigest(),
        },
        "bindings": observation["bindings"],
        "completion": completion,
    }
    trace_directory.mkdir(parents=True, exist_ok=True)
    atomic_json(trace_directory / "run.json", document)
    relative = f"jobs/{directory.name}/trace"
    result = {
        "trace": {
            "run": relative + "/run.json",
            "events": relative + "/events.jsonl",
            "complete": completion["complete"],
            "reason": completion["reason"],
            "plan_signature": document["plan_signature"],
            "source_target_sha256": session.source_target_sha256,
            "executed_target_sha256": session.executed_target_sha256,
        },
        "launch_identity": session.launch_identity,
        "source_target_sha256": session.source_target_sha256,
        "executed_target_sha256": session.executed_target_sha256,
        "input_mode": launch["input_mode"],
        "execution": execution_metadata(launch["cwd"], session.experiment),
        "experiment_uid": session.experiment["uid"] if session.experiment else None,
        "metadata_protected": bool(session.experiment),
    }
    if stream_error is not None or cleanup_error is not None:
        result["error"] = str(cleanup_error or stream_error)[:4096]
    return JobOutcome(state, result, cleanup_pending=cleanup_error is not None)
