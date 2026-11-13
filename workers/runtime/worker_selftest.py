"""Known offline tool probes, invoked only after native package verification.

This is an operator/installer entry, never a worker RPC. The native caller must
verify the signed package against existing trust and keep its tree private.
Passing these trusted fixtures does not establish production target isolation.
"""
import sys
sys.dont_write_bytecode = True
import argparse
import base64
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time

from worker_common import PROTOCOL, safe_env, sha256_file
from worker_runtime import Runtime


def ssh_probe_environment():
    environment = safe_env()
    # Win32-OpenSSH initializes its ProgramData paths even for ssh-keygen.
    # Keep this extra OS path scoped to the fixed SSH probe, not target jobs.
    program_data = next((value for key, value in os.environ.items() if key.casefold() == "programdata"), None)
    if not program_data or not Path(program_data).is_absolute():
        raise ValueError("Windows ProgramData is missing from the SSH probe environment")
    environment["ProgramData"] = program_data
    return environment


def checked_file(root, manifest, name):
    if name.startswith(("/", "\\")) or "\\" in name or any(part in {"", ".", ".."} for part in name.split("/")):
        raise ValueError("Invalid self-test package member")
    path = root / name
    for parent in [path, *path.parents]:
        if parent == root.parent:
            break
        metadata = parent.lstat()
        if parent.is_symlink() or getattr(metadata, "st_file_attributes", 0) & 0x400:
            raise ValueError("Linked self-test package members are forbidden")
    expected = manifest["files"][name]
    if not path.is_file() or path.stat().st_size != expected["size"] or sha256_file(path) != expected["sha256"]:
        raise ValueError("Self-test package member changed")
    return path


def architecture_test(root, manifest, state, architecture):
    directory = state / architecture
    directory.mkdir()
    target = checked_file(root, manifest, f"runtime/selftest/{architecture}/wukong-probe.exe")
    echo = subprocess.run([str(target), "--echo"], cwd=directory, env=safe_env(), capture_output=True, timeout=15)
    if echo.returncode or echo.stdout != b"wukong-worker-selftest\n":
        raise ValueError("Native self-test probe could not execute")
    runtime = Runtime(directory, trusted=True)
    image = runtime.dispatch("file.put", {"path": target.name, "data_base64": base64.b64encode(target.read_bytes()).decode(), "executable": True})
    dumped = directory / "probe.dmp"
    process = subprocess.run([str(directory / image["path"]), "--dump", str(dumped)], cwd=directory, env=safe_env(), capture_output=True, timeout=20)
    if process.returncode:
        raise ValueError("Offline dump generation failed")
    runtime.dispatch("file.put", {"path": "probe.dmp", "data_base64": base64.b64encode(dumped.read_bytes()).decode()})
    payload = b"CRASH: offline package self-test"
    input_ = runtime.dispatch("file.put", {"path": "replay-input", "data_base64": base64.b64encode(payload).decode()})
    job = runtime.dispatch("poc.verify", dict(target=image["path"], input=input_["path"], args=["--replay", "@@"], trials=3,
        timeout_secs=3, call_id="offline-selftest", producer_agent_id="offline-producer", verifier_agent_id="offline-verifier"))
    deadline = time.monotonic() + 90
    while job["state"] not in {"completed", "failed", "cancelled", "timed_out", "lost"}:
        if time.monotonic() >= deadline:
            runtime.dispatch("job.cancel", {"job_id": job["job_id"]})
            raise ValueError("Offline replay self-test exceeded deadline")
        time.sleep(0.05)
        job = runtime.dispatch("job.status", {"job_id": job["job_id"]})
    result = job.get("result", {})
    if (job["state"] != "completed" or result.get("passed") is not True or result.get("successes") != 3
            or result.get("attempts") != 3 or result.get("target", {}).get("sha256") != image["sha256"]
            or result.get("input", {}).get("sha256") != input_["sha256"]
            or len(result.get("results", [])) != 3
            or any(row.get("crash_signature") != "exception:0xc0000005" for row in result["results"])):
        raise ValueError("Offline native replay self-test failed")
    if runtime.active_resources():
        raise ValueError("Offline self-test retained active resources")
    return {"architecture": architecture, "target_sha256": image["sha256"], "dump_sha256": sha256_file(dumped), "replay_trials": 3}


def run(package, state):
    if os.name != "nt":
        raise ValueError("Offline Windows self-test requires Windows")
    package, state = Path(package).resolve(), Path(state).resolve()
    if state == package or package in state.parents:
        raise ValueError("Self-test state must stay outside the signed deployment")
    with (package / "manifest.json").open("rb") as incoming:
        raw = incoming.read(4 * 1024 * 1024 + 1)
    if len(raw) > 4 * 1024 * 1024:
        raise ValueError("Self-test manifest exceeds limit")
    manifest = json.loads(raw)
    if manifest["runtime_protocol"] != PROTOCOL:
        raise ValueError("Incompatible self-test package")
    state.mkdir(parents=True, exist_ok=True)
    attempt = Path(tempfile.mkdtemp(prefix="offline-selftest-", dir=state))
    ssh = checked_file(package, manifest, manifest["entrypoints"]["ssh_keygen"])
    try:
        subprocess.run([str(ssh), "-q", "-t", "ed25519", "-N", "", "-f", str(attempt / "ssh-probe")],
                       cwd=attempt, env=ssh_probe_environment(), check=True, capture_output=True, timeout=20)
    finally:
        # Only the dummy key just created for this attempt; never host/auth keys.
        (attempt / "ssh-probe").unlink(missing_ok=True)
        (attempt / "ssh-probe.pub").unlink(missing_ok=True)
    results = [architecture_test(package, manifest, attempt, architecture) for architecture in ["x64", "x86"]]
    report = {"passed": True, "scope": "offline_dependencies_and_trusted_fixtures", "runtime_protocol": PROTOCOL,
              "signed_manifest_sha256": sha256_file(package / "manifest.json"), "architectures": results,
              "production_isolation_verified": False, "state_directory": str(attempt)}
    (attempt / "result.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--package", type=Path, required=True)
    parser.add_argument("--state", type=Path, required=True)
    args = parser.parse_args()
    print(json.dumps(run(args.package, args.state)))


if __name__ == "__main__":
    main()
