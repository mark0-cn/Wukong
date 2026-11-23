"""Real multiprocess parity for the derived-runtime publication lock."""

import hashlib
import json
import multiprocessing
from pathlib import Path
import queue
import subprocess
import sys
import tempfile
import time
import unittest
from unittest import mock

import wukong_worker.execution.launch as launch


def _call_patched_runtime(base, spec, parent, results, calls, entered, release):
    def patchelf(command, **_kwargs):
        with calls.get_lock():
            calls.value += 1
        entered.set()
        if not release.wait(10):
            raise AssertionError("test did not release the critical section")
        return subprocess.CompletedProcess(command, 0)

    with (
        mock.patch.object(launch, "PUBLIC_LAUNCH_CACHE", parent),
        mock.patch.object(launch.subprocess, "run", side_effect=patchelf),
    ):
        try:
            target = launch.patched_runtime(base, spec)
            results.put(("ok", str(target), hashlib.sha256(target.read_bytes()).hexdigest()))
        except BaseException as error:  # Transport child failures to the test process.
            results.put(("error", type(error).__name__, str(error)))


class PatchedRuntimeLockTests(unittest.TestCase):
    def setUp(self):
        if sys.platform != "linux":
            self.skipTest("Linux flock semantics across spawned processes")
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.addCleanup(self._make_tree_writable)
        self.root = Path(self.temporary.name)
        self.base = self.root / "base"
        (self.base / "lib").mkdir(parents=True)
        (self.base / "target").write_bytes(b"trusted target fixture")
        (self.base / "lib/ld.so").write_bytes(b"trusted loader fixture")
        (self.base / "lib/libc.so").write_bytes(b"trusted libc fixture")
        self.parent = self.root / "cache"
        self.parent.mkdir()
        cache_patch = mock.patch.object(launch, "PUBLIC_LAUNCH_CACHE", self.parent)
        cache_patch.start()
        self.addCleanup(cache_patch.stop)
        self.spec = {
            "target": "target",
            "library_path": "lib",
            "loader": "lib/ld.so",
            "files": {
                "target": "a" * 64,
                "lib/ld.so": "b" * 64,
                "lib/libc.so": "c" * 64,
            },
        }
        key = hashlib.sha256(json.dumps(self.spec["files"], sort_keys=True).encode()).hexdigest()
        self.destination = self.parent / key
        self.lock = self.parent / (key + ".lock")

    def _make_tree_writable(self):
        if not self.root.exists():
            return
        for path in sorted(self.root.rglob("*"), reverse=True):
            path.chmod(0o755 if path.is_dir() else 0o644)
        self.root.chmod(0o755)

    @staticmethod
    def _queued(results):
        pending = []
        while True:
            try:
                pending.append(results.get_nowait())
            except queue.Empty:
                return pending

    @staticmethod
    def _stop(process):
        if process.is_alive():
            process.terminate()
        process.join(5)

    def _start(self, process):
        process.start()
        self.addCleanup(self._stop, process)

    def _join(self, processes, results):
        for process in processes:
            process.join(10)
            if process.is_alive():
                self.fail(self._queued(results))
            self.assertEqual(process.exitcode, 0)
        return [results.get(timeout=1) for _ in processes]

    def test_first_use_is_serialized_and_published_once(self):
        context = multiprocessing.get_context("spawn")
        calls = context.Value("i", 0)
        entered = context.Event()
        release = context.Event()
        results = context.Queue()

        arguments = (self.base, self.spec, self.parent, results, calls, entered, release)
        first = context.Process(target=_call_patched_runtime, args=arguments)
        second = context.Process(target=_call_patched_runtime, args=arguments)
        self._start(first)
        if not entered.wait(5):
            self.fail(self._queued(results))
        lock_inode = self.lock.stat().st_ino
        self._start(second)
        time.sleep(0.2)
        observed = (second.is_alive(), calls.value, self.destination.exists())
        release.set()
        completed = self._join([first, second], results)
        self.assertEqual(observed, (True, 1, False))
        self.assertTrue(all(item[0] == "ok" for item in completed), completed)
        self.assertEqual(len({item[1] for item in completed}), 1)
        self.assertEqual(len({item[2] for item in completed}), 1)
        self.assertEqual(calls.value, 1)
        self.assertEqual(self.lock.stat().st_ino, lock_inode)
        self.assertFalse((self.destination / "identity.json").stat().st_mode & 0o222)

        cached = launch.patched_runtime(self.base, self.spec)
        self.assertEqual(str(cached), completed[0][1])
        self.assertEqual(calls.value, 1)
        self.assertEqual(
            {path.name for path in self.parent.iterdir()}, {self.destination.name, self.lock.name}
        )

    def test_failure_cleans_only_temporary_output_and_preserves_identity_checks(self):
        calls = 0

        def patchelf(command, **_kwargs):
            nonlocal calls
            calls += 1
            if calls == 1:
                raise subprocess.CalledProcessError(1, command, stderr=b"injected")
            return subprocess.CompletedProcess(command, 0)

        patchelf_patch = mock.patch.object(launch.subprocess, "run", side_effect=patchelf)
        patchelf_patch.start()
        self.addCleanup(patchelf_patch.stop)
        with self.assertRaises(subprocess.CalledProcessError):
            launch.patched_runtime(self.base, self.spec)
        self.assertTrue(self.lock.exists())
        lock_inode = self.lock.stat().st_ino
        self.assertFalse(self.destination.exists())
        self.assertEqual([path.name for path in self.parent.iterdir()], [self.lock.name])

        target = launch.patched_runtime(self.base, self.spec)
        self.assertEqual(calls, 2)
        self.assertEqual(self.lock.stat().st_ino, lock_inode)
        target.chmod(0o755)
        target.write_bytes(b"tampered")
        target.chmod(0o555)
        with self.assertRaisesRegex(ValueError, "cached runtime was modified"):
            launch.patched_runtime(self.base, self.spec)


if __name__ == "__main__":
    unittest.main()
