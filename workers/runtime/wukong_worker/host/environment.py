"""Child-process environment policy; controller secrets never cross this boundary."""

import os
import re

ENV_KEYS = {
    "PATH",
    "SystemRoot",
    "WINDIR",
    "TEMP",
    "TMP",
    "PATHEXT",
    "COMSPEC",
    "HOME",
    "USERPROFILE",
    "LANG",
    "LC_ALL",
    "JAVA_HOME",
    "WUKONG_GHIDRA_HEADLESS",
    "WUKONG_GHIDRA_SCRIPT_DIR",
}


def safe_env(extra: dict[str, object] | None = None) -> dict[str, str]:
    # Windows enumerates environment names in upper case; SYSTEMROOT is needed
    # by Winsock even when Python itself launches successfully without it.
    allowed = {key.upper() for key in ENV_KEYS}
    env = {k: v for k, v in os.environ.items() if k.upper() in allowed}
    env.update({"LANG": "C.UTF-8", "PYTHONUNBUFFERED": "1"})
    for k, v in (extra or {}).items():
        if not re.fullmatch(r"(?:AFL_[A-Z0-9_]+|QEMU_[A-Z0-9_]+|ASAN_OPTIONS|UBSAN_OPTIONS)", k):
            raise ValueError("environment variable not permitted: " + k)
        if len(str(v)) > 4096:
            raise ValueError("environment value too long")
        env[k] = str(v)
    return env
