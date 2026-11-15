"""Bounded process execution; OS controls and ownership live in platform."""

from pathlib import Path
import os
import subprocess
import threading
import time
from wukong_worker.artifacts.common import MAX_FILE, tail
from wukong_worker.platform.process_owner import ProcessResources


def run_process(
    argv,
    cwd,
    env,
    timeout_secs,
    output_limit,
    log_prefix,
    cancel_file=None,
    experiment=None,
    stdin_bytes=None,
):
    if (
        not isinstance(argv, list)
        or not argv
        or len(argv) > 128
        or any(not isinstance(a, str) or "\0" in a or len(a) > 65536 for a in argv)
    ):
        raise ValueError("argv must be an array of 1..128 bounded strings")
    if stdin_bytes is not None and len(stdin_bytes) > MAX_FILE:
        raise ValueError("stdin payload exceeds 32 MiB")
    start = time.monotonic()
    credential_options = {}
    if experiment:
        import wukong_worker.platform.isolation as isolation

        credential_options = isolation.credentials(experiment)
        env = {**env, "HOME": experiment["work"], "TMPDIR": experiment["work"]}
    with ProcessResources() as resources:
        process = resources.spawn(
            argv,
            cwd=cwd,
            env=env,
            stdin=subprocess.PIPE if stdin_bytes is not None else subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            **credential_options,
        )
        counts = {"stdout": 0, "stderr": 0}

        def feed_stdin():
            # Bounded best-effort write: a target that never reads must not wedge
            # the call; the timeout and cancel paths own termination.
            try:
                process.stdin.write(stdin_bytes)
                process.stdin.flush()
            except (BrokenPipeError, OSError):
                pass
            try:
                process.stdin.close()
            except OSError:
                pass

        if stdin_bytes is not None:
            threading.Thread(target=feed_stdin, daemon=True).start()

        def drain(name, stream):
            with open(str(log_prefix) + "." + name, "wb") as log:
                while True:
                    data = stream.read(8192)
                    if not data:
                        break
                    remaining = max(0, output_limit - counts[name])
                    if remaining:
                        log.write(data[:remaining])
                    counts[name] += len(data)
            stream.close()

        readers = [
            threading.Thread(target=drain, args=(name, getattr(process, name)), daemon=True)
            for name in counts
        ]
        for thread in readers:
            thread.start()
        state = "completed"
        try:
            while process.poll() is None:
                if cancel_file and Path(cancel_file).exists():
                    state = "cancelled"
                    break
                if time.monotonic() - start >= timeout_secs:
                    state = "timed_out"
                    break
                time.sleep(0.03)
        finally:
            resources.close()
            for thread in readers:
                thread.join(timeout=5)
        return {
            "state": state,
            "exit_code": process.returncode,
            "signal": -process.returncode
            if process.returncode is not None and process.returncode < 0 and os.name != "nt"
            else None,
            "duration_secs": round(time.monotonic() - start, 4),
            "stdout": tail(str(log_prefix) + ".stdout"),
            "stderr": tail(str(log_prefix) + ".stderr"),
            "output_bytes": counts,
            "output_truncated": any(v > output_limit for v in counts.values()),
        }
