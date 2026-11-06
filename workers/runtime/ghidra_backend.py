"""Read-only headless Ghidra adapter with identity-bound projects/results."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import time
import uuid
import bridge as runtime


def digest(path):
    value = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for block in iter(lambda: stream.read(1048576), b""):
            value.update(block)
    return value.hexdigest()


def identity(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":")).encode()).hexdigest()


def query(root, params):
    target = runtime.confined(root, params["target"], True)
    actual_hash = digest(target)
    if params.get("target_sha256") != actual_hash:
        raise ValueError("Ghidra target SHA256 does not match uploaded target")
    headless = Path(os.environ.get("RE_AGENT_GHIDRA_HEADLESS", "/opt/ghidra/support/analyzeHeadless")).resolve()
    scripts = Path(os.environ.get("RE_AGENT_GHIDRA_SCRIPT_DIR", "/opt/re-agent/ghidra")).resolve()
    script = scripts / "ReAgentBridge.java"
    if not headless.is_file() or not script.is_file():
        raise ValueError("Ghidra worker unavailable: install Ghidra and ReAgentBridge.java or set RE_AGENT_GHIDRA_HEADLESS / RE_AGENT_GHIDRA_SCRIPT_DIR")
    properties = headless.parent.parent / "Ghidra/application.properties"
    properties_text = properties.read_text(encoding="utf-8")
    version = next((line.split("=", 1)[1].strip() for line in properties_text.splitlines() if line.startswith("application.version=")), "unknown")
    timeout = runtime.bounded_int(params.get("timeout_secs"), 110, 5, 295)
    analysis_timeout = runtime.bounded_int(params.get("analysis_timeout_secs"), min(90, timeout - 1), 1, 290)
    max_cpu = runtime.bounded_int(params.get("max_cpu"), 2, 1, 16)
    operation = dict(params["query"])
    if operation.get("operation") not in {"functions", "decompile", "xrefs"}:
        raise ValueError("unsupported Ghidra operation")
    if operation["operation"] in {"functions", "xrefs"}:
        operation["limit"] = runtime.bounded_int(operation.get("limit"), 25, 1, 5000)
    if operation["operation"] == "functions":
        operation["offset"] = runtime.bounded_int(operation.get("offset"), 0, 0, 100000000)
    binding = {"schema": 1, "target_sha256": actual_hash, "version": version, "properties_sha256": digest(properties),
               "launcher_sha256": digest(headless), "bridge_sha256": digest(script), "analysis_timeout_secs": analysis_timeout, "max_cpu": max_cpu}
    analysis_key = identity(binding)
    cache_key = identity({"analysis": binding, "query": operation})
    directory = root / "ghidra/results" / cache_key
    directory.mkdir(parents=True, exist_ok=True)
    cache = directory / "result.json"
    if cache.is_file():
        result = runtime.load_json(cache)
        if result["target_sha256"] != actual_hash or result["cache_key"] != cache_key:
            raise ValueError("Ghidra cache identity mismatch")
        result["cached"] = True
        return result
    project = root / "ghidra/projects" / analysis_key
    project.mkdir(parents=True, exist_ok=True)
    lock_path = project / "worker.lock"
    deadline = time.monotonic() + min(timeout, 60)
    while True:
        try:
            with lock_path.open("x", encoding="utf-8") as lock:
                json.dump({"owner_pid": os.getpid(), "owner_identity": runtime.process_identity(os.getpid()), "created_at": time.time()}, lock)
            break
        except FileExistsError:
            if cache.is_file():
                result = runtime.load_json(cache)
                result["cached"] = True
                return result
            if time.monotonic() >= deadline:
                raise ValueError("Ghidra project busy or owner lost; inspect worker.lock and running analyzer before removing stale lock")
            time.sleep(0.1)
    try:
        if cache.is_file():
            result = runtime.load_json(cache)
            result["cached"] = True
            return result
        ready = project / "analysis-complete.json"
        snapshot = project / "target.bin"
        if not ready.is_file():
            if snapshot.exists():
                snapshot.chmod(0o644)
            shutil.copyfile(target, snapshot)
            if digest(snapshot) != actual_hash:
                raise ValueError("Ghidra target changed during staging")
            snapshot.chmod(0o444)
        work = directory / uuid.uuid4().hex
        work.mkdir()
        request, response = work / "request.json", work / "response.json"
        runtime.atomic_json(request, {"target_sha256": actual_hash, "query": operation})
        runtime.atomic_json(work / "binding.json", {"analysis": binding, "query": operation})
        argv = [str(headless), str(project), "analysis"]
        if ready.is_file():
            argv += ["-process", "target.bin", "-noanalysis"]
        else:
            argv += ["-import", str(snapshot), "-overwrite"]
        argv += ["-analysisTimeoutPerFile", str(analysis_timeout), "-max-cpu", str(max_cpu), "-scriptPath", str(scripts),
                 "-postScript", "ReAgentBridge.java", str(request), str(response)]
        execution = runtime.run_process(argv, work, runtime.safe_env(), timeout, 4 * 1048576, work / "output")
        runtime.atomic_json(work / "execution.json", execution)
        if execution["state"] != "completed" or execution["exit_code"] != 0:
            raise ValueError("Ghidra execution failed: %s; logs: %s" % (execution["state"], work.relative_to(root).as_posix()))
        if not response.is_file() or response.stat().st_size > 16 * 1048576:
            raise ValueError("Ghidra response absent/too large; inspect " + work.relative_to(root).as_posix())
        data = runtime.load_json(response)
        if data.get("ok") is not True:
            raise ValueError("Ghidra bridge error: " + str(data.get("error", "unknown"))[:2048])
        if data.get("target_sha256") != actual_hash or digest(target) != actual_hash:
            raise ValueError("Ghidra target identity changed during analysis")
        if data.get("artifact_path"):
            path = Path(data["artifact_path"]).resolve()
            if work not in path.parents or not path.is_file():
                raise ValueError("Ghidra returned an invalid artifact path")
            data["artifact_path"] = path.relative_to(root).as_posix()
        runtime.atomic_json(ready, binding)
        artifacts = [runtime.artifact(root, path) for path in sorted(work.iterdir()) if path.is_file()]
        result = {"cache_key": cache_key, "target_sha256": actual_hash, "tool_version": version, "cached": False,
                  "data": data, "artifacts": artifacts, "stdout_path": (work / "output.stdout").relative_to(root).as_posix(),
                  "stderr_path": (work / "output.stderr").relative_to(root).as_posix()}
        runtime.atomic_json(cache, result)
        return result
    finally:
        lock_path.unlink(missing_ok=True)
