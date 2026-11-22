"""Multiprocess and failure parity for experiment UID allocation."""

import json
import multiprocessing
from pathlib import Path
import sys
import tempfile
import time
import unittest
from unittest import mock

import wukong_worker.platform.isolation as isolation


def _allocate(root, results):
    try:
        results.put(isolation.allocate_uid(root))
    except BaseException as error:  # Transport child failures to the test process.
        results.put((type(error).__name__, str(error)))


class UidAllocatorLockTests(unittest.TestCase):
    def setUp(self):
        if sys.platform != "linux":
            self.skipTest("Linux UID, flock and fork semantics")
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.lock = self.root / "uid-allocator.lock"
        self.state = self.root / "uid-allocator.json"

    def _registration(self, **kwargs):
        registration = mock.patch.object(isolation, "ensure_uid_entries", **kwargs)
        registration.start()
        self.addCleanup(registration.stop)
        passwd = mock.patch("pwd.getpwuid", return_value=object())
        passwd.start()
        self.addCleanup(passwd.stop)
        return registration

    def _join(self, processes, results):
        for process in processes:
            process.join(10)
            self.assertFalse(process.is_alive(), "allocator remained blocked")
            self.assertEqual(process.exitcode, 0)
        return [results.get(timeout=1) for _ in processes]

    def test_multiprocess_allocations_are_unique_and_never_lose_updates(self):
        self._registration(return_value=None)
        context = multiprocessing.get_context("fork")
        results = context.Queue()
        processes = [context.Process(target=_allocate, args=(self.root, results)) for _ in range(6)]
        for process in processes:
            process.start()
        allocated = self._join(processes, results)
        self.assertTrue(all(isinstance(value, int) for value in allocated), allocated)
        self.assertEqual(sorted(allocated), list(range(20000, 20006)))
        self.assertEqual(json.loads(self.state.read_text()), {"next": 20006})
        self.assertEqual(self.lock.stat().st_mode & 0o777, 0o600)
        self.assertEqual(self.state.stat().st_mode & 0o777, 0o600)
        self.assertFalse(self.state.with_suffix(".tmp").exists())

    def test_registration_remains_inside_the_blocking_critical_section(self):
        context = multiprocessing.get_context("fork")
        entered = context.Event()
        release = context.Event()
        results = context.Queue()

        def pause_registration():
            entered.set()
            if not release.wait(10):
                raise AssertionError("test did not release registration")

        self._registration(side_effect=pause_registration)
        first = context.Process(target=_allocate, args=(self.root, results))
        second = context.Process(target=_allocate, args=(self.root, results))
        first.start()
        self.assertTrue(entered.wait(5))
        self.assertEqual(json.loads(self.state.read_text()), {"next": 20001})
        lock_inode = self.lock.stat().st_ino
        second.start()
        time.sleep(0.2)
        blocked = second.is_alive()
        release.set()
        allocated = self._join([first, second], results)
        self.assertTrue(blocked)
        self.assertEqual(sorted(allocated), [20000, 20001])
        self.assertEqual(json.loads(self.state.read_text()), {"next": 20002})
        self.assertEqual(self.lock.stat().st_ino, lock_inode)
        self.assertEqual(self.lock.stat().st_mode & 0o777, 0o600)

    def test_failures_preserve_consumed_uids_locks_and_retry_behavior(self):
        self.lock.touch(mode=0o644)
        self.lock.chmod(0o644)
        lock_inode = self.lock.stat().st_ino
        passwd = mock.patch("pwd.getpwuid", return_value=object())
        passwd.start()
        self.addCleanup(passwd.stop)
        with mock.patch.object(
            isolation, "ensure_uid_entries", side_effect=RuntimeError("injected registration")
        ):
            with self.assertRaisesRegex(RuntimeError, "injected registration"):
                isolation.allocate_uid(self.root)
        self.assertEqual(json.loads(self.state.read_text()), {"next": 20001})
        self.assertEqual(self.lock.stat().st_ino, lock_inode)
        self.assertEqual(self.lock.stat().st_mode & 0o777, 0o600)

        with mock.patch.object(isolation, "ensure_uid_entries", return_value=None):
            self.assertEqual(isolation.allocate_uid(self.root), 20001)
        self.assertEqual(json.loads(self.state.read_text()), {"next": 20002})

        self.state.write_text("not-json")
        with mock.patch.object(isolation, "ensure_uid_entries", return_value=None):
            with self.assertRaises(json.JSONDecodeError):
                isolation.allocate_uid(self.root)
            self.state.write_text(json.dumps({"next": 1000000000}))
            with self.assertRaisesRegex(ValueError, "exhausted"):
                isolation.allocate_uid(self.root)
            self.assertEqual(json.loads(self.state.read_text()), {"next": 1000000000})
            self.state.write_text(json.dumps({"next": 30000}))
            with mock.patch.object(
                isolation.os, "replace", side_effect=OSError("injected replace")
            ):
                with self.assertRaisesRegex(OSError, "injected replace"):
                    isolation.allocate_uid(self.root)
            self.assertEqual(json.loads(self.state.read_text()), {"next": 30000})
            self.assertEqual(isolation.allocate_uid(self.root), 30000)
            self.assertEqual(json.loads(self.state.read_text()), {"next": 30001})


if __name__ == "__main__":
    unittest.main()
