"""Read-only headless Ghidra adapter with identity-bound projects/results."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import time
import uuid
from contextlib import contextmanager
from filelock import FileLock, Timeout
from worker_common import artifact, atomic_json, bounded_int, confined, load_json, safe_env, sha256_file as digest
from worker_processes import run_process


def identity(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":")).encode()).hexdigest()


def require_idle(project):
    if (project / "analysis.active.json").exists():
        raise ValueError("prior Ghidra analysis outcome is unknown; inspect analysis.active.json and the analyzer before clearing the claim")


@contextmanager
def analysis_claim(project, argv):
    # A kernel lock alone is insufficient if its owner dies before its analyzer.
    # Leave the claim on abnormal exit; never infer that a child exited from PID loss.
    require_idle(project)
    marker = project / "analysis.active.json"
    atomic_json(marker, {"owner_pid": os.getpid(), "argv": argv, "created_at": time.time()})
    yield
    marker.unlink()

def query(root, params):
    target = confined(root, params["target"], True)
    actual_hash = digest(target)
    if params.get("target_sha256") != actual_hash:
        raise ValueError("Ghidra target SHA256 does not match uploaded target")
    headless = Path(os.environ.get("WUKONG_GHIDRA_HEADLESS", "/opt/ghidra/support/analyzeHeadless")).resolve()
    scripts = Path(os.environ.get("WUKONG_GHIDRA_SCRIPT_DIR", "/opt/wukong/ghidra")).resolve()
    script = scripts / "WukongBridge.java"
    if not headless.is_file() or not script.is_file():
        raise ValueError("Ghidra worker unavailable: install Ghidra and WukongBridge.java or set WUKONG_GHIDRA_HEADLESS / WUKONG_GHIDRA_SCRIPT_DIR")
    properties = headless.parent.parent / "Ghidra/application.properties"
    properties_text = properties.read_text(encoding="utf-8")
    version = next((line.split("=", 1)[1].strip() for line in properties_text.splitlines() if line.startswith("application.version=")), "unknown")
    timeout = bounded_int(params.get("timeout_secs"), 110, 5, 590)
    analysis_timeout = bounded_int(params.get("analysis_timeout_secs"), min(90, timeout - 1), 1, 290)
    max_cpu = bounded_int(params.get("max_cpu"), 2, 1, 16)
    operation = dict(params["query"])
    if operation.get("operation") not in {"functions", "decompile", "xrefs", "instructions"}:
        raise ValueError("unsupported Ghidra operation")
    if operation["operation"] in {"functions", "xrefs", "instructions"}:
        operation["limit"] = bounded_int(operation.get("limit"), 25, 1, 5000)
    if operation["operation"] in {"functions", "instructions"}:
        operation["offset"] = bounded_int(operation.get("offset"), 0, 0, 100000000)
    binding = {"schema": 1, "target_sha256": actual_hash, "version": version, "properties_sha256": digest(properties),
               "launcher_sha256": digest(headless), "bridge_sha256": digest(script), "analysis_timeout_secs": analysis_timeout, "max_cpu": max_cpu}
    analysis_key = identity(binding)
    cache_key = identity({"analysis": binding, "query": operation})
    directory = root / "ghidra/results" / cache_key
    directory.mkdir(parents=True, exist_ok=True)
    cache = directory / "result.json"
    if cache.is_file():
        result = load_json(cache)
        if result["target_sha256"] != actual_hash or result["cache_key"] != cache_key:
            raise ValueError("Ghidra cache identity mismatch")
        result["cached"] = True
        return result
    project = root / "ghidra/projects" / analysis_key
    project.mkdir(parents=True, exist_ok=True)
    lock = FileLock(str(project / "worker.lock"))
    deadline = time.monotonic() + min(timeout, 60)
    while True:
        try:
            lock.acquire(timeout=0)
            break
        except Timeout:
            if cache.is_file():
                result = load_json(cache)
                result["cached"] = True
                return result
            if time.monotonic() >= deadline:
                raise ValueError("Ghidra project is busy; the analyzer still owns the project lock")
            time.sleep(0.1)
    try:
        if cache.is_file():
            result = load_json(cache)
            result["cached"] = True
            return result
        require_idle(project)
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
        atomic_json(request, {"target_sha256": actual_hash, "query": operation})
        atomic_json(work / "binding.json", {"analysis": binding, "query": operation})
        argv = [str(headless), str(project), "analysis"]
        if ready.is_file():
            argv += ["-process", "target.bin", "-noanalysis"]
        else:
            argv += ["-import", str(snapshot), "-overwrite"]
        argv += ["-analysisTimeoutPerFile", str(analysis_timeout), "-max-cpu", str(max_cpu), "-scriptPath", str(scripts),
                 "-postScript", "WukongBridge.java", str(request), str(response)]
        with analysis_claim(project, argv):
            execution = run_process(argv, work, safe_env(), timeout, 4 * 1048576, work / "output")
        atomic_json(work / "execution.json", execution)
        if execution["state"] != "completed" or execution["exit_code"] != 0:
            raise ValueError("Ghidra execution failed: %s; logs: %s" % (execution["state"], work.relative_to(root).as_posix()))
        if not response.is_file() or response.stat().st_size > 16 * 1048576:
            raise ValueError("Ghidra response absent/too large; inspect " + work.relative_to(root).as_posix())
        data = load_json(response)
        if data.get("ok") is not True:
            raise ValueError("Ghidra bridge error: " + str(data.get("error", "unknown"))[:2048])
        if data.get("target_sha256") != actual_hash or digest(target) != actual_hash:
            raise ValueError("Ghidra target identity changed during analysis")
        if data.get("artifact_path"):
            path = Path(data["artifact_path"]).resolve()
            if work not in path.parents or not path.is_file():
                raise ValueError("Ghidra returned an invalid artifact path")
            data["artifact_path"] = path.relative_to(root).as_posix()
        atomic_json(ready, binding)
        artifacts = [artifact(root, path) for path in sorted(work.iterdir()) if path.is_file()]
        result = {"cache_key": cache_key, "target_sha256": actual_hash, "tool_version": version, "cached": False,
                  "data": data, "artifacts": artifacts, "stdout_path": (work / "output.stdout").relative_to(root).as_posix(),
                  "stderr_path": (work / "output.stderr").relative_to(root).as_posix()}
        atomic_json(cache, result)
        return result
    finally:
        lock.release()
