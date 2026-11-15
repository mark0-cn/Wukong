"""Replay trials and crash/expectation interpretation, without ledger writes."""

from pathlib import Path
import hashlib
import os
from wukong_worker.artifacts.common import artifact, confined
from wukong_worker.artifacts.metadata import atomic_json
from wukong_worker.platform.environment import safe_env
from wukong_worker.execution.processes import run_process


def crash_signature(result):
    code = result.get("exit_code")
    if result.get("state") != "completed":
        return None
    if result.get("location_signature"):
        return result["location_signature"]
    if result.get("signal"):
        return "signal:" + str(result["signal"])
    if (
        os.name == "nt"
        and code is not None
        and (code & 0xFFFFFFFF) in {0xC0000005, 0xC000001D, 0xC0000094, 0xC00000FD, 0xC0000409}
    ):
        return "exception:" + hex(code & 0xFFFFFFFF)
    return None


def expectation_matches(result, expected):
    if result["state"] != "completed":
        return False
    allowed = {"exit_code", "signal", "stdout_contains", "stderr_contains"}
    if set(expected or {}) - allowed:
        raise ValueError("unsupported verification expectation")
    expected = {key: value for key, value in (expected or {}).items() if value is not None}
    if not expected:
        return crash_signature(result) is not None
    return all(
        (str(value) in result[key[:-9]]) if key.endswith("_contains") else result.get(key) == value
        for key, value in expected.items()
    )


def replay(root, directory, spec, input_bytes, index):
    target = confined(root, spec["target"], True)
    sandbox = directory / ("trial-%04d" % index)
    sandbox.mkdir()
    input_path = sandbox / "input.bin"
    input_path.write_bytes(input_bytes)
    input_path.chmod(0o444)
    experiment = None
    cwd = sandbox
    if spec.get("isolated") and not spec.get("launch_spec"):
        import wukong_worker.platform.isolation as isolation

        experiment = isolation.sandbox(root, isolation.allocate_uid(root))
        target = isolation.stage_file(target, experiment, "target.bin", True)
        public_input = Path(experiment["readonly"]) / "input.bin"
        public_input.write_bytes(input_bytes)
        public_input.chmod(0o444)
        input_path = public_input
        isolation.seal(experiment)
        cwd = Path(experiment["work"])
    input_mode = spec.get("input_mode", "argv")
    if input_mode not in {"argv", "stdin"}:
        raise ValueError("input_mode must be 'argv' or 'stdin'")
    if spec.get("launch_spec"):
        from wukong_worker.execution.launch import resolve

        launch = resolve(root, spec, spec.get("isolated", False), input_path)
        experiment = launch["experiment"]
        result = run_process(
            launch["argv"],
            launch["cwd"],
            launch["env"],
            spec["timeout_secs"],
            spec["output_limit"],
            sandbox / "output",
            directory / "cancel",
            experiment,
            stdin_bytes=input_bytes if input_mode == "stdin" else None,
        )
        result["launch_identity"] = launch["identity"]
    elif input_mode == "stdin":
        # Pipe the payload bytes into the target's stdin; any extra args are
        # passed through verbatim (no @@ file-path substitution).
        argv = [str(target)] + [str(arg) for arg in spec.get("args", []) if arg != "@@"]
        result = run_process(
            argv,
            cwd,
            safe_env(spec.get("env")),
            spec["timeout_secs"],
            spec["output_limit"],
            sandbox / "output",
            directory / "cancel",
            experiment,
            stdin_bytes=input_bytes,
        )
    else:
        argv = [str(target)] + [
            str(input_path) if arg == "@@" else str(arg) for arg in spec.get("args", ["@@"])
        ]
        result = run_process(
            argv,
            cwd,
            safe_env(spec.get("env")),
            spec["timeout_secs"],
            spec["output_limit"],
            sandbox / "output",
            directory / "cancel",
            experiment,
        )
    result["experiment_uid"] = experiment["uid"] if experiment else None
    result["metadata_protected"] = bool(experiment)
    result["input_sha256"] = hashlib.sha256(input_bytes).hexdigest()
    result["input_mode"] = input_mode
    if (
        spec.get("kind") == "minimize"
        and not spec.get("allow_signal_only", False)
        and crash_signature(result)
    ):
        from wukong_worker.debugger.crash import replay_snapshot

        try:
            snapshot = replay_snapshot(root, spec, sandbox / "input.bin", directory / "cancel")
        except Exception as error:
            snapshot = {"signature_strength": "unresolved", "error": str(error)[:1024]}
        atomic_json(sandbox / "crash-snapshot.json", snapshot)
        # A debugger-only failure is not evidence for the original native crash.
        if snapshot.get("signal_number") == result.get("signal"):
            result["location_signature"] = snapshot.get("location_signature")
        result["snapshot"] = artifact(root, sandbox / "crash-snapshot.json")
    result["crash_signature"] = crash_signature(result)
    result["trial"] = index
    return result
