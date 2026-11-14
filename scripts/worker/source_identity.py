"""Bind standalone native builds to their actual source and locked inputs."""
import hashlib
from pathlib import Path
import subprocess

from scripts._lib.repository import ROOT
from scripts.worker.paths import regular

SOURCES = ["workers/windows", "workers/package", "workers/protocol", "workers/analysis", "scripts/worker"]


def identity():
    digest = hashlib.sha256()
    for directory in SOURCES:
        for path in sorted((ROOT / directory).rglob("*")):
            if path.suffix not in {".py", ".rs", ".proto", ".patch", ".c", ".h", ".toml", ".lock", ".json", ".txt"} or not path.is_file():
                continue
            if any(part in {"__pycache__", "target", "build", "dist"} or part.endswith(".egg-info")
                   for part in path.relative_to(ROOT).parts):
                continue
            name = path.relative_to(ROOT).as_posix().encode("ascii")
            digest.update(len(name).to_bytes(4, "little"))
            digest.update(name)
            with regular(path).open("rb") as incoming:
                digest.update(hashlib.file_digest(incoming, "sha256").digest())
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    dirty = bool(subprocess.check_output(["git", "status", "--porcelain", "--untracked-files=all", "--", *SOURCES], cwd=ROOT))
    return dict(source_revision=revision, source_tree_sha256=digest.hexdigest(), source_dirty=dirty)
