"""Worker artifact confinement, hashes and bounded file reads."""

from pathlib import Path
import hashlib
import re


MAX_FILE = 32 * 1024 * 1024


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
    return {
        "path": path.relative_to(root).as_posix(),
        "sha256": sha256_file(path),
        "size": path.stat().st_size,
    }


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
    if path.name in {
        "spec.json",
        "status.json",
        "heartbeat.json",
        "ledger.lock",
        "supervisor-start.json",
        "cancel",
        "session.json",
        "processes.json",
        "supervisor.log",
    }:
        raise ValueError("path is private worker metadata; use status APIs")
    return path
