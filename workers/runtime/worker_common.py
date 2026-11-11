"""Protocol values, confined paths and atomic metadata IO."""
from pathlib import Path
import hashlib
import json
import os
import re
import time
import uuid

PROTOCOL = 2
MAX_RPC = 48 * 1024 * 1024
MAX_FILE = 32 * 1024 * 1024
TERMINAL = {"completed", "failed", "cancelled", "timed_out", "lost"}
ENV_KEYS = {"PATH", "SystemRoot", "WINDIR", "TEMP", "TMP", "PATHEXT", "COMSPEC", "HOME", "USERPROFILE", "LANG", "LC_ALL", "JAVA_HOME", "WUKONG_GHIDRA_HEADLESS", "WUKONG_GHIDRA_SCRIPT_DIR"}
BRIDGE_PATH = Path(__file__).with_name("bridge.py").resolve()

class WorkerError(ValueError):
    def __init__(self, kind, message):
        super().__init__(message)
        self.kind = kind

def rpc_error(error):
    kind = getattr(error, "kind", "invalid_request" if isinstance(error, (ValueError, KeyError, TypeError)) else "internal")
    return {"code": -32000, "message": str(error)[:4096], "data": {"kind": kind}}

def safe_env(extra=None):
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

def atomic_json(path, data):
    path = Path(path)
    temporary = path.with_name(path.name + "." + uuid.uuid4().hex + ".tmp")
    temporary.write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
    # Windows: a concurrent reader holds the destination without
    # FILE_SHARE_DELETE, so os.replace fails transiently with WinError 5.
    # Retry briefly; the replacement itself stays atomic.
    for attempt in range(5):
        try:
            os.replace(temporary, path)
            return
        except OSError:
            if attempt == 4:
                raise
            time.sleep(0.02 * (attempt + 1))

def load_json(path):
    # Windows: a concurrent atomic replacement can fail the open with a
    # transient sharing violation; retry briefly like atomic_json does.
    path = Path(path)
    for attempt in range(5):
        try:
            return json.loads(path.read_text(encoding="utf-8"))
        except OSError:
            if attempt == 4:
                raise
            time.sleep(0.02 * (attempt + 1))

def confined(root, relative, exists=False):
    root = Path(root).resolve()
    value = Path(str(relative))
    if value.is_absolute() or value.drive or ".." in value.parts:
        raise ValueError("path must be relative to the worker state directory")
    value = (root / value).resolve()
    if value != root and root not in value.parents:
        raise ValueError("path escapes worker state directory")
    if exists and not value.exists():
        raise ValueError("worker path does not exist: " + str(relative))
    return value

def job_dir(root, job_id):
    if not re.fullmatch(r"[0-9a-f]{32}", str(job_id)):
        raise ValueError("invalid job_id")
    return confined(root, "jobs/" + job_id, True)

def bounded_int(value, default, minimum, maximum):
    value = int(value if value is not None else default)
    if not minimum <= value <= maximum:
        raise ValueError("numeric parameter outside allowed range")
    return value

def parse_address(value):
    """Accept the controller's interchangeable integer spellings for module-relative
    addresses: JSON integers, plain decimal strings and 0x-hex strings."""
    if isinstance(value, bool) or isinstance(value, float):
        raise ValueError("address must be a nonnegative integer or 0x-hex string")
    if isinstance(value, int):
        parsed = value
    elif isinstance(value, str):
        text = value.strip().replace("_", "")
        if not text:
            raise ValueError("address must be a nonnegative integer or 0x-hex string")
        parsed = int(text, 16) if text.lower().startswith("0x") else int(text, 10)
    else:
        raise ValueError("address must be a nonnegative integer or 0x-hex string")
    if not 0 <= parsed < 2**64:
        raise ValueError("address outside the 64-bit module-relative range")
    return parsed

def tail(path, limit=8192):
    if not Path(path).is_file():
        return ""
    with open(path, "rb") as stream:
        stream.seek(max(0, Path(path).stat().st_size - limit))
        return stream.read(limit).decode("utf-8", errors="replace")

def sha256_file(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def artifact(root, path):
    path = Path(path).resolve()
    if root != path and root not in path.parents:
        raise ValueError("artifact outside worker root")
    if not path.is_file():
        return None
    return {"path": path.relative_to(root).as_posix(), "sha256": sha256_file(path), "size": path.stat().st_size}

def public_file(root, relative):
    root = Path(root).resolve()
    path = confined(root, relative, True)
    parts = path.relative_to(root).parts
    if not parts or parts[0] not in {"files", "jobs", "debug", "fuzz", "ghidra", "interact"}:
        raise ValueError("path is private worker metadata")
    if parts[0] == "files":
        # Uploaded payload names are data, even when named status.json. Path
        # confinement has already resolved aliases before selecting this root.
        return path
    if parts[0] == "ghidra" and (len(parts) < 2 or parts[1] != "results"):
        raise ValueError("path is private Ghidra project metadata")
    if path.name in {"spec.json", "status.json", "heartbeat.json", "cancel", "session.json", "supervisor.log"}:
        raise ValueError("path is private worker metadata; use status APIs")
    return path
