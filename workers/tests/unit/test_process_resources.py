"""Process ownership across exceptions, independent scopes and daemon shutdown."""

import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import unittest
from unittest.mock import Mock, patch

from wukong_worker.execution.processes import run_process
from wukong_worker.host.runtime import Runtime
from wukong_worker.resources.processes import ProcessResources


class ProcessResourceTests(unittest.TestCase):
    def test_exception_closes_only_owned_processes_and_close_is_idempotent(self):
        with ProcessResources() as other:
            survivor = other.spawn([sys.executable, "-c", "import time; time.sleep(60)"])
            owner = ProcessResources()
            with self.assertRaisesRegex(RuntimeError, "executor failed"):
                with owner:
                    child = owner.spawn([sys.executable, "-c", "import time; time.sleep(60)"])
                    raise RuntimeError("executor failed")
            self.assertIsNotNone(child.returncode)
            self.assertIsNone(survivor.poll())
            owner.close()
            with self.assertRaisesRegex(ValueError, "closed"):
                owner.spawn([sys.executable, "-c", "pass"])

    def test_failed_cleanup_retains_ownership_for_retry(self):
        process = Mock(pid=42)
        owner = ProcessResources()
        with patch("wukong_worker.resources.processes.subprocess.Popen", return_value=process):
            with patch("wukong_worker.execution.processes.attach_windows_job"):
                owner.spawn(["fixture"])
        with patch("wukong_worker.execution.processes.terminate_tree") as terminate:
            terminate.side_effect = [OSError("retry close"), None]
            with self.assertRaises(OSError):
                owner.close()
            owner.close()
            owner.close()
            self.assertEqual(terminate.call_count, 2)

    def test_cleanup_failure_does_not_abandon_other_owned_children(self):
        owner = ProcessResources()
        first, second = Mock(pid=41), Mock(pid=42)
        with patch(
            "wukong_worker.resources.processes.subprocess.Popen", side_effect=[first, second]
        ):
            with patch("wukong_worker.execution.processes.attach_windows_job"):
                owner.spawn(["first"])
                owner.spawn(["second"])
        with patch("wukong_worker.execution.processes.terminate_tree") as terminate:
            terminate.side_effect = [OSError("retry"), None, None]
            with self.assertRaises(OSError):
                owner.close()
            second.wait.assert_called_once()
            first.wait.assert_not_called()
            owner.close()
            self.assertEqual(terminate.call_count, 3)

    def test_durable_children_are_reaped_without_waiting_for_the_next_spawn(self):
        owner = ProcessResources()
        child = owner.spawn([sys.executable, "-c", "pass"], durable=True)
        owner.wait()
        self.assertEqual(child.returncode, 0)
        if os.name == "posix":
            with self.assertRaises(ChildProcessError):
                os.waitpid(child.pid, os.WNOHANG)

    def test_run_process_exception_does_not_abandon_child(self):
        children = []
        original = subprocess.Popen

        def spawn(*args, **kwargs):
            child = original(*args, **kwargs)
            children.append(child)
            return child

        with tempfile.TemporaryDirectory() as directory:
            with patch("wukong_worker.resources.processes.subprocess.Popen", side_effect=spawn):
                with patch(
                    "wukong_worker.execution.processes.time.monotonic",
                    side_effect=[0, OSError("clock failed")],
                ):
                    with self.assertRaisesRegex(OSError, "clock failed"):
                        run_process(
                            [sys.executable, "-c", "import time; time.sleep(60)"],
                            directory,
                            dict(os.environ),
                            60,
                            1024,
                            Path(directory) / "log",
                        )
            self.assertEqual(len(children), 1)
            self.assertIsNotNone(children[0].returncode)

    def test_interactive_publication_failure_reaps_child(self):
        from wukong_worker.debugger.interactive import InteractSession

        children = []
        original = subprocess.Popen

        def spawn(*args, **kwargs):
            child = original(*args, **kwargs)
            children.append(child)
            return child

        with tempfile.TemporaryDirectory() as directory:
            with patch("wukong_worker.resources.processes.subprocess.Popen", side_effect=spawn):
                with patch.object(
                    InteractSession, "persist", side_effect=OSError("publication failed")
                ):
                    with self.assertRaisesRegex(OSError, "publication failed"):
                        InteractSession(
                            Path(directory),
                            {"argv": [sys.executable, "-c", "import time; time.sleep(60)"]},
                        )
            self.assertEqual(len(children), 1)
            self.assertIsNotNone(children[0].returncode)

    def test_shutdown_cannot_race_an_active_request(self):
        with tempfile.TemporaryDirectory() as directory:
            runtime = Runtime(Path(directory), trusted=True)
            entered, release = threading.Event(), threading.Event()

            def active(_):
                entered.set()
                release.wait(5)
                return {}

            runtime.handlers["ping"] = active
            thread = threading.Thread(target=runtime.dispatch, args=("ping", {}))
            thread.start()
            self.assertTrue(entered.wait(5))
            try:
                with self.assertRaisesRegex(ValueError, "active resources"):
                    runtime.dispatch("worker.shutdown", {"request_id": "busy-shutdown"})
                self.assertFalse(runtime.stopping)
            finally:
                release.set()
                thread.join(5)
            self.assertEqual(runtime.shutdown({})["state"], "shutdown")
            with self.assertRaisesRegex(ValueError, "shutting down"):
                runtime.dispatch("ping", {})
