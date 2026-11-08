"""Capabilities."""
from pathlib import Path
import platform
import shutil
import subprocess
import sys
from worker_common import PROTOCOL, safe_env

def doctor(root, trusted, isolated):
    tools = {}
    for name in ["gdb", "afl-fuzz", "afl-qemu-trace", "afl-qemu-bridge", "afl-tmin", "gcc"]:
        executable = shutil.which(name)
        version = None
        if executable and name in {"gdb", "gcc"}:
            try:
                version = subprocess.run([executable, "--version"], env=safe_env(), capture_output=True, timeout=5).stdout.decode(errors="replace").splitlines()[0][:256]
            except (OSError, subprocess.TimeoutExpired, IndexError):
                pass
        tools[name] = {"available": bool(executable), "path": executable, "version": version}
    linux = sys.platform.startswith("linux")
    core_pattern = None
    if linux:
        try:
            core_pattern = Path("/proc/sys/kernel/core_pattern").read_text().strip()[:512]
        except OSError:
            pass
    return {"protocol": PROTOCOL, "platform": platform.platform(), "python": platform.python_version(),
            "worker_root": str(root), "isolation": "trusted_local" if trusted else "external_worker",
            "core_pattern": core_pattern, "external_core_handler": bool(core_pattern and core_pattern.startswith("|")),
            "metadata_protected_from_targets": isolated, "unique_experiment_uids": isolated,
            "tools": tools, "capabilities": {"process_jobs": True, "persistent_jobs": True,
            "debug_gdb_mi": tools["gdb"]["available"], "fuzz_qemu": linux and tools["afl-fuzz"]["available"] and (tools["afl-qemu-trace"]["available"] or tools["afl-qemu-bridge"]["available"]),
            "crash_replay": True, "input_minimize": True, "poc_verify": True, "shell_exec": True,
            "formats": ["ELF"] if linux else [], "native_target_execution": True}}
