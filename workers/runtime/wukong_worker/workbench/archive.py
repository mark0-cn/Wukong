"""Volume snapshots in a disposable helper with no Docker socket or host keys."""

import json
import sys
import tarfile
from pathlib import Path


def export(root, archive):
    def selected(info):
        if info.name in {"./endpoint.json", "./daemon.lock"}:
            return None
        return None if not (info.isfile() or info.isdir() or info.issym() or info.islnk()) else info

    with tarfile.open(archive, "w") as stream:
        stream.add(root, arcname=".", filter=selected)


def restore(root, archive):
    with tarfile.open(archive, "r") as stream:
        members = stream.getmembers()
        if len(members) > 200000 or sum(m.size for m in members) > 64 * 1024**3:
            raise ValueError("volume archive exceeds limits")
        stream.extractall(root, members=members, filter="data")
    # An imported PID, socket, or daemon ownership record is never live authority.
    for pattern in ["debug/*/session.json", "interact/*/session.json", "jobs/*/status.json"]:
        for path in root.glob(pattern):
            value = json.loads(path.read_text())
            if value.get("state") in {
                "running",
                "queued",
                "starting",
                "closing",
                "recovery_required",
                "lost",
            }:
                value.update(
                    state="lost",
                    cleanup_confirmed=True,
                    cleanup_pending=False,
                    reconciliation="imported_snapshot_requires_new_execution",
                )
            for key in ["pid", "gdb_pid", "daemon_pid", "supervisor_pid", "supervisor_identity"]:
                value.pop(key, None)
            path.write_text(json.dumps(value))
    # Imported process journals are evidence of the source, never authority to
    # signal PIDs or delete scratch paths in the destination environment.
    for kind in ["debug", "interact"]:
        for path in (root / kind).glob("*/processes.json"):
            path.unlink()
    for path in root.glob("*.sock"):
        path.unlink()
    for name in ["daemon.json", "bridge-daemon.json", "endpoint.json", "daemon.lock"]:
        path = root / name
        if path.exists():
            path.unlink()


if __name__ == "__main__":
    action, root, archive = sys.argv[1:]
    if action not in {"export", "restore"}:
        raise ValueError("invalid archive action")
    globals()[action](Path(root), Path(archive))
