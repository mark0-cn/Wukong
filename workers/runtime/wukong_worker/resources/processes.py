"""Ownership of child processes; never reap or signal another owner's child.

Execution scopes close process groups. Durable supervisors outlive RPC clients
and are waited by their owning daemon without cancelling their work. Orphaned
grandchildren belong to the container init (or VM init), not waitpid(-1).
"""

import subprocess
import threading
import time


class ProcessResources:
    def __init__(self):
        self._lock = threading.RLock()
        self._children = {}
        self._closed = False

    def spawn(self, argv, *, durable=False, **options):
        from wukong_worker.execution.processes import attach_windows_job, detached_options

        with self._lock:
            if self._closed:
                raise ValueError("process resource owner is closed")
            process = subprocess.Popen(argv, **detached_options(), **options)
            self._children[process.pid] = process
            if not durable:
                attach_windows_job(process)
            else:
                # Only wait on this exact Popen. Never use waitpid(-1), which
                # could consume exit statuses belonging to concurrent executors.
                threading.Thread(target=self._reap, args=(process,), daemon=True).start()
            return process

    def _reap(self, process):
        process.wait()
        with self._lock:
            if self._children.get(process.pid) is process:
                del self._children[process.pid]

    def wait(self, timeout=5):
        """Drain supervisors after their durable jobs have become terminal."""
        deadline = time.monotonic() + timeout
        with self._lock:
            children = list(self._children.values())
        for process in children:
            process.wait(timeout=max(0, deadline - time.monotonic()))
            with self._lock:
                if self._children.get(process.pid) is process:
                    del self._children[process.pid]

    def close(self):
        from wukong_worker.execution.processes import terminate_tree

        with self._lock:
            self._closed = True
            failures = []
            for pid, process in list(self._children.items()):
                try:
                    terminate_tree(process)
                    process.wait(timeout=5)
                except Exception as error:
                    # Try other owned children too; keep failures for retry.
                    failures.append(error)
                else:
                    del self._children[pid]
            if failures:
                raise failures[0]

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()
