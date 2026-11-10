"""Shared artifact checks; execution and service policy stay with their callers."""
from dataclasses import dataclass
import json
from pathlib import Path
from worker_common import confined, sha256_file as digest


@dataclass(frozen=True)
class LaunchDocument:
    raw: bytes
    spec: dict
    base: Path

    def member(self, name):
        if not isinstance(name, str) or not name or Path(name).is_absolute() or ".." in Path(name).parts:
            raise ValueError("invalid launch member")
        value = (self.base / name).resolve()
        if not value.is_relative_to(self.base) or not value.exists():
            raise ValueError("launch member escapes or is missing")
        return value

    def check_files(self, max_file=None, max_total=None):
        files = self.spec.get("files", {})
        target = self.spec.get("target")
        if not isinstance(files, dict) or not target or target not in files or len(files) > 1024:
            raise ValueError("launch file identity missing")
        total = 0
        for name, expected in files.items():
            path = self.member(name)
            size = path.stat().st_size
            total += size
            if (max_file is not None and size > max_file) or (max_total is not None and total > max_total):
                raise ValueError("launch files exceed identity budget")
            if digest(path) != expected:
                raise ValueError("launch input changed: " + name)
        return files


def read_launch(root, path):
    path = confined(root, path, True)
    if path.stat().st_size > 65536:
        raise ValueError("launch spec exceeds 64 KiB")
    raw = path.read_bytes()
    if len(raw) > 65536:
        raise ValueError("launch spec exceeds 64 KiB")
    spec = json.loads(raw)
    if not isinstance(spec, dict) or spec.get("schema_version") != 1:
        raise ValueError("unsupported launch schema")
    base = confined(root, spec["base"], True)
    if not base.is_dir():
        raise ValueError("launch base must be a directory")
    return LaunchDocument(raw, spec, base)
