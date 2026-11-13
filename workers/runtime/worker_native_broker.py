"""Fixed native broker health; it never enables target execution."""
import json
import os
import re
import subprocess

from worker_common import safe_env, sha256_file
from worker_windows_paths import local_path
from worker_processes import detached_options


def doctor(identity):
    if os.name != "nt":
        return {"available": False, "reason": "native broker requires Windows"}
    try:
        keys = ["CLIENT", "CLIENT_SHA256", "INSTALLATION", "PARENT_PID"]
        client, digest, installation, parent = [os.environ["WUKONG_NATIVE_BROKER_" + key] for key in keys]
        if (not re.fullmatch(r"[0-9a-f]{64}", digest) or not re.fullmatch(r"[0-9a-f]{32}", installation)
                or not parent.isascii() or not parent.isdigit() or not 0 < int(parent) <= 0xffffffff or str(int(parent)) != parent):
            raise ValueError("invalid native broker startup binding")
        client = local_path(client)
        if not client.is_file() or sha256_file(client) != digest:
            raise ValueError("native broker client differs from the sealed worker")
        result = subprocess.run([str(client), "broker-health", installation, parent], env=safe_env(),
            capture_output=True, timeout=4, **detached_options())
        if result.returncode:
            raise ValueError("native broker health failed: " + result.stderr[-384:].decode("utf-8", errors="replace"))
        if len(result.stdout) > 4096:
            raise ValueError("native broker health exceeds 4 KiB")
        response = json.loads(result.stdout)
        if (not isinstance(response, dict) or set(response) != {"schema", "identity", "production_execution", "scope"}
                or type(response["schema"]) is not int or response["schema"] != 1
                or response["identity"] != identity or response["production_execution"] is not False
                or response["scope"] != "native_broker_channel_only"):
            raise ValueError("native broker health binding or execution scope differs")
        return {"available": True, **response}
    except (OSError, ValueError, KeyError, subprocess.TimeoutExpired) as error:
        return {"available": False, "reason": str(error)[:512]}
