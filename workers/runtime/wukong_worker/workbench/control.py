"""Host-invoked management probes; not exposed as target-controlled shell commands."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

# Direct execution by the host does not install a second runtime package.
if __name__ == "__main__" and not __package__:
    sys.dont_write_bytecode = True
    sys.path.insert(0, str(Path(__file__).resolve().parents[2]))

from wukong_worker.artifacts.metadata import load_json
from wukong_worker.host.environment import safe_env
from wukong_worker.resources import TERMINAL


def idle(root):
    from wukong_worker.resources.jobs import JobManager

    active = []
    jobs = JobManager(root, False, True)
    for path in (root / "jobs").glob("*/status.json"):
        status = jobs.status(path.parent.name)
        if status["state"] not in TERMINAL or status.get("cleanup_pending"):
            active.append({"kind": "job", "id": path.parent.name, "state": status["state"]})
    for kind in ["debug", "interact"]:
        records = set((root / kind).glob("*/session.json"))
        records.update(p.parent / "session.json" for p in (root / kind).glob("*/processes.json"))
        for path in records:
            state = load_json(path) if path.exists() else {}
            journal = path.parent / "processes.json"
            confirmed = state.get("state") == "closed" or (
                state.get("state") == "lost" and state.get("cleanup_confirmed")
            )
            if not confirmed or (journal.exists() and load_json(journal)["observed"] != "closed"):
                active.append(
                    {
                        "kind": kind,
                        "id": path.parent.name,
                        "state": state.get("state", "recovery_required"),
                    }
                )
    return {"idle": not active, "active": active}


def doctor(root):
    from wukong_worker.host.capabilities import doctor as inventory
    from wukong_worker.host.runtime import Runtime

    checks = {}
    env = safe_env()
    env["TERM"] = "xterm"

    def check(name, argv, cwd=None):
        try:
            result = subprocess.run(
                argv, cwd=cwd, env=env, input=b"probe\n", capture_output=True, timeout=60
            )
            checks[name] = {
                "passed": result.returncode == 0,
                "exit_code": result.returncode,
                "output": (result.stdout + result.stderr).decode(errors="replace")[-4096:],
            }
        except (OSError, subprocess.TimeoutExpired) as error:
            checks[name] = {"passed": False, "error": str(error)}

    with tempfile.TemporaryDirectory(prefix="workbench-doctor-", dir=root) as directory:
        directory = Path(directory)
        source = directory / "probe.c"
        source.write_text(
            "#include <stdio.h>\nint main(){char b[16]; if(!fgets(b,sizeof b,stdin))return 1; puts(b); return 0;}\n"
        )
        binary = directory / "probe"
        check("compile", ["gcc", "-g", "-o", str(binary), str(source)])
        check("native", [str(binary)])
        check(
            "gdb",
            [
                "gdb",
                "-nx",
                "-nh",
                "-batch",
                "-ex",
                "set auto-load off",
                "-ex",
                "break main",
                "-ex",
                "run",
                "-ex",
                "info registers",
                str(binary),
            ],
        )
        check("i386_compile", ["gcc", "-m32", "-o", str(directory / "probe32"), str(source)])
        check("i386_run", [str(directory / "probe32")])
        check(
            "pwntools",
            [
                sys.executable,
                "-c",
                "from pwn import *; p=process(["
                + repr(str(binary))
                + "]); p.sendline(b'probe'); assert b'probe' in p.recvall(timeout=2); assert p.poll()==0",
            ],
        )
        for name in ["pwninit", "patchelf", "seccomp-tools", "pwndbg", "eu-unstrip"]:
            check(name, [name, "--version"])
        check(
            "python_analysis",
            [
                sys.executable,
                "-c",
                "import angr,z3,capstone,unicorn,elftools,pygdbmi; print('imports OK')",
            ],
        )
        ghidra = os.environ.get("WUKONG_GHIDRA_HEADLESS")
        if ghidra and Path(ghidra).is_file():
            check(
                "ghidra",
                [
                    ghidra,
                    str(directory),
                    "doctor-project",
                    "-import",
                    str(binary),
                    "-analysisTimeoutPerFile",
                    "20",
                    "-deleteProject",
                ],
            )
        else:
            checks["ghidra"] = {"passed": False, "error": "headless Ghidra unavailable"}
        # Bind the real handler registry against disposable state. Constructing
        # Runtime(root) here would recover live sessions owned by another daemon.
        registry_root = directory / "registry"
        registry_root.mkdir(mode=0o700)
        probe = Runtime(registry_root, trusted=False)
        observed = inventory(root, False, True, probe.registrations)
    return {
        "passed": all(c["passed"] for c in checks.values()),
        "checks": checks,
        "inventory": observed,
    }


if __name__ == "__main__":
    action = sys.argv[1]
    root = Path(sys.argv[2]).resolve()
    root.mkdir(parents=True, exist_ok=True)
    if action not in {"idle", "doctor"}:
        raise ValueError("unknown management probe")
    print(json.dumps(idle(root) if action == "idle" else doctor(root)))
