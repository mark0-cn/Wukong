"""Durable cleanup intent for owned Linux process groups, never PID-only killing."""

import os
import hashlib
from pathlib import Path
import signal
import shutil
import time
from wukong_worker.artifacts.metadata import atomic_json, load_json


def boot_identity():
    return Path("/proc/sys/kernel/random/boot_id").read_text().strip()


def group_members(group):
    members = []
    for entry in Path("/proc").iterdir():
        if not entry.name.isdecimal():
            continue
        try:
            fields = (entry / "stat").read_text().rsplit(")", 1)[1].split()
            if int(fields[2]) == group and fields[0] != "Z":
                members.append(int(entry.name))
        except FileNotFoundError:
            continue
    return members


def remove_scratch(experiment):
    if not experiment:
        return
    namespace = hashlib.sha256(experiment["state_root"].encode()).hexdigest()[:24]
    expected_parent = Path("/tmp/wukong-executions") / namespace
    directory = Path(experiment["root"])
    if (
        directory.parent != expected_parent
        or len(directory.name) != 32
        or any(c not in "0123456789abcdef" for c in directory.name)
        or directory.is_symlink()
    ):
        raise ValueError("scratch cleanup ownership mismatch")
    if directory.exists():
        if directory.resolve() != directory or directory.stat().st_uid != os.geteuid():
            raise ValueError("scratch cleanup path or owner changed")
        shutil.rmtree(directory)


class ProcessJournal:
    def __init__(self, path, experiment=None):
        self.path = Path(path)
        self.state = {
            "desired": "live",
            "observed": "preparing",
            "uncertain_start": True,
            "children": [],
            "boot_id": boot_identity(),
            "pid_namespace": os.readlink("/proc/self/ns/pid"),
        }
        self.state["scratch"] = experiment
        atomic_json(self.path, self.state)

    def starting(self):
        self.state["observed"] = "preparing"
        self.state["uncertain_start"] = True
        atomic_json(self.path, self.state)

    def spawn_failed(self):
        # Popen returned no owned child; do not turn a rejected spawn into an
        # uncertain crash window. An actual crash keeps the starting marker.
        self.state.update(observed="live", uncertain_start=False)
        atomic_json(self.path, self.state)

    def spawned(self, process):
        from wukong_worker.execution.processes import process_identity

        identity = process_identity(process.pid)
        self.state["children"].append(
            {"pid": process.pid, "identity": identity, "group": process.pid}
        )
        self.state["observed"] = "live"
        self.state["uncertain_start"] = False
        atomic_json(self.path, self.state)

    def check_owned(self, pid):
        from wukong_worker.execution.processes import process_identity

        child = next(child for child in self.state["children"] if child["pid"] == pid)
        current = process_identity(pid)
        # A live owner retains its process-group authority after reaping the
        # leader. A replacement leader is never that owner's child. Restart
        # reconciliation below deliberately requires stronger positive proof.
        if current != "missing" and (current == "unknown" or current != child["identity"]):
            raise RuntimeError("owned process creation identity changed")

    def closing(self):
        self.state.update(desired="closed", observed="closing")
        atomic_json(self.path, self.state)

    def closed(self):
        deadline = time.monotonic() + 5
        while any(group_members(child["group"]) for child in self.state["children"]):
            if time.monotonic() >= deadline:
                raise RuntimeError("owned process group cleanup remains unconfirmed")
            time.sleep(0.02)
        remove_scratch(self.state.get("scratch"))
        self.state.update(desired="closed", observed="closed")
        atomic_json(self.path, self.state)

    @staticmethod
    def reconcile(path):
        from wukong_worker.execution.processes import process_identity

        path = Path(path)
        state = load_json(path)
        if state["observed"] == "closed":
            return True
        preparing = state.get("uncertain_start", state["observed"] == "preparing")
        state.update(desired="closed", observed="closing")
        atomic_json(path, state)
        try:
            if state["boot_id"] != boot_identity():
                remove_scratch(state.get("scratch"))
                state["observed"] = "closed"
                return True
            if preparing or state["pid_namespace"] != os.readlink("/proc/self/ns/pid"):
                return False
            for child in state["children"]:
                pid, group = child["pid"], child["group"]
                if not isinstance(pid, int) or pid <= 1 or group != pid:
                    return False
                current = process_identity(pid)
                members = group_members(group)
                if current == "missing" and not members:
                    continue
                if current in {"missing", "unknown"} or current != child["identity"]:
                    return False
                if os.getpgid(pid) != group:
                    return False
                os.killpg(group, signal.SIGKILL)
                deadline = time.monotonic() + 5
                while group_members(group) and time.monotonic() < deadline:
                    time.sleep(0.02)
                if group_members(group):
                    return False
            remove_scratch(state.get("scratch"))
            state["observed"] = "closed"
            return True
        finally:
            if state["observed"] != "closed":
                state["observed"] = "recovery_required"
            atomic_json(path, state)
