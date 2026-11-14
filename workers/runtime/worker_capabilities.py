"""Capabilities."""
from pathlib import Path
import importlib.util
import json
import os
import platform
import shutil
import subprocess
import sys
from worker_common import PROTOCOL, safe_env
from worker_catalog import capabilities


def _ghidra_ready(linux, execution_ready):
    if not linux or not execution_ready:
        return False
    launcher = Path(os.environ.get("WUKONG_GHIDRA_HEADLESS", "/opt/ghidra/support/analyzeHeadless"))
    scripts = Path(os.environ.get("WUKONG_GHIDRA_SCRIPT_DIR", "/opt/wukong/ghidra"))
    if not (launcher.is_file() and os.access(launcher, os.X_OK)
            and (scripts / "WukongBridge.java").is_file()
            and (launcher.parent.parent / "Ghidra/application.properties").is_file()):
        return False
    java = shutil.which("java")
    if not java:
        return False
    try:
        return subprocess.run([java, "-version"], env=safe_env(), capture_output=True, timeout=5).returncode == 0
    except (OSError, subprocess.TimeoutExpired):
        return False


def _python_module(name):
    try:
        return importlib.util.find_spec(name) is not None
    except (ImportError, ValueError):
        return False


def doctor(root, trusted, isolated, registrations):
    tools = {}
    for name in ["gdb", "afl-fuzz", "afl-qemu-trace", "afl-qemu-bridge", "afl-tmin", "gcc",
                 "one_gadget", "patchelf", "ropper", "pwninit", "pwndbg", "tmux", "strace", "ltrace", "seccomp-tools", "socat", "gdbserver", "eu-unstrip"]:
        executable = shutil.which(name)
        version = None
        if executable and name in {"gdb", "gcc"}:
            try:
                version = subprocess.run([executable, "--version"], env=safe_env(), capture_output=True, timeout=5).stdout.decode(errors="replace").splitlines()[0][:256]
            except (OSError, subprocess.TimeoutExpired, IndexError):
                pass
        tools[name] = {"available": bool(executable), "path": executable, "version": version}
    linux = sys.platform.startswith("linux")
    execution_ready = trusted or isolated
    core_pattern = None
    if linux:
        try:
            core_pattern = Path("/proc/sys/kernel/core_pattern").read_text().strip()[:512]
        except OSError:
            pass
    python_toolchain = {
        "pwntools": _python_module("pwn"),
        "pyelftools": _python_module("elftools"),
        "pygdbmi": _python_module("pygdbmi"),
        "picire": all(_python_module(name) for name in ("picire", "chardet", "inators")),
        "z3": _python_module("z3"),
        "angr": _python_module("angr"),
        "unicorn": _python_module("unicorn"),
        "capstone": _python_module("capstone"),
    }
    workbench=None
    launch=Path(root)/"files/challenge/launch.json"
    if launch.is_file():
        try:
            spec=json.loads(launch.read_text())
            workbench={"launch_spec":"files/challenge/launch.json","challenge_id":spec["challenge_id"],"target":str(Path(spec["base"])/spec["target"]) if spec.get("target") else None,"input_mode":spec["input_mode"],"service":spec["service"],"runtime_confirmed":False}
        except (OSError,ValueError,KeyError) as error:workbench={"error":str(error)}
    ready = {
        "files": True, "control": True, "debug_session": linux, "execution": execution_ready, "interactive": execution_ready,
        "debugger": execution_ready and linux and tools["gdb"]["available"] and python_toolchain["pygdbmi"],
        "fuzz": execution_ready and linux and tools["afl-fuzz"]["available"] and (tools["afl-qemu-trace"]["available"] or tools["afl-qemu-bridge"]["available"]),
        "symbolic": execution_ready and python_toolchain["angr"], "emulation": python_toolchain["unicorn"],
        "minimize": execution_ready and python_toolchain["picire"], "elf": python_toolchain["pyelftools"],
        "cyclic": python_toolchain["pwntools"], "core": linux and python_toolchain["pwntools"],
        "ghidra": _ghidra_ready(linux, execution_ready),
        "network": bool(workbench and workbench.get("target") and workbench.get("service",{}).get("mode") in {"local_service","remote"}),
    }
    methods = {name: {"ready": ready[operation.readiness]} for name, operation in registrations.items()}
    advertised = capabilities(registrations, ready)
    advertised.update({"debug_dbgeng": False, "input_minimize": ready["minimize"] and ready["debugger"],
                       "process_mapping_observations": linux,
                       "formats": ["ELF"] if linux and execution_ready else ["PE"] if execution_ready and os.name == "nt" else []})
    return {"methods":methods,"workbench":workbench,"protocol": PROTOCOL, "os": platform.system().lower(), "platform": platform.platform(), "python": platform.python_version(),
            "worker_root": str(root), "isolation": "trusted_local" if trusted else "external_worker",
            "core_pattern": core_pattern, "external_core_handler": bool(core_pattern and core_pattern.startswith("|")),
            "metadata_protected_from_targets": isolated, "unique_experiment_uids": isolated,
            "tools": tools, "python_toolchain": python_toolchain,
            "capabilities": advertised}
