"""Cold daemon startup must outlast the bounded Windows identity query."""
from concurrent.futures import ThreadPoolExecutor
from filelock import FileLock
from pathlib import Path
import subprocess
import sys
import tempfile
import time
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from worker_processes import terminate_tree
from worker_transport import ensure_daemon, relay_call


class DaemonStartupTests(unittest.TestCase):
    def test_slow_initialization_remains_one_owned_daemon_and_reuses_endpoint(self):
        runtime = Path(__file__).resolve().parents[2] / "runtime"
        children = []
        original_popen = subprocess.Popen

        def spawn(*args, **kwargs):
            child = original_popen(*args, **kwargs)
            children.append(child)
            return child

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            script = root / "slow-daemon.py"
            script.write_text(
                "import os,sys,time\n"
                "from pathlib import Path\n"
                f"sys.path.insert(0,{str(runtime)!r})\n"
                "from worker_transport import serve\n"
                "Path(sys.argv[sys.argv.index('--state')+1],'fixture.pid').write_text(str(os.getpid()))\n"
                "time.sleep(6)\n"
                "serve(Path(sys.argv[sys.argv.index('--state')+1]),True)\n",
                encoding="utf-8",
            )
            state = root / "state"
            state.mkdir()
            endpoint = None
            try:
                with patch("worker_transport.BRIDGE_PATH", script), patch(
                    "worker_transport.subprocess",
                    SimpleNamespace(Popen=spawn, DEVNULL=subprocess.DEVNULL),
                ):
                    started = time.monotonic()
                    with ThreadPoolExecutor(max_workers=4) as callers:
                        endpoints = list(callers.map(lambda _: ensure_daemon(state, True), range(4)))
                    endpoint = endpoints[0]
                    self.assertTrue(all(item == endpoint for item in endpoints))
                    self.assertGreaterEqual(time.monotonic() - started, 6)
                    self.assertEqual(ensure_daemon(state, True), endpoint)
                self.assertEqual(len(children), 1)
                self.assertEqual(endpoint["pid"], int((state / "fixture.pid").read_text()))
                self.assertIsNone(children[0].poll())
                duplicate = subprocess.run(
                    [sys.executable, "-c",
                     "import sys; from pathlib import Path; "
                     f"sys.path.insert(0,{str(runtime)!r}); "
                     "from worker_transport import serve; "
                     f"serve(Path({str(state)!r}),True)"],
                    capture_output=True, timeout=10,
                )
                self.assertNotEqual(duplicate.returncode, 0)
                self.assertIn(b"filelock", duplicate.stderr)
                self.assertEqual(ensure_daemon(state, True), endpoint)
                with FileLock(state / "startup.guard", timeout=0):
                    pass
            finally:
                try:
                    if endpoint:
                        relay_call(endpoint, {"id": 1, "method": "worker.shutdown"}, timeout=5)
                finally:
                    for child in children:
                        try:
                            child.wait(timeout=5)
                        except subprocess.TimeoutExpired:
                            terminate_tree(child)

    def test_failed_daemon_reports_its_exit_without_waiting_for_startup_deadline(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            script = root / "failed-daemon.py"
            script.write_text("print('fixture-startup-diagnostic',flush=True)\nraise SystemExit(19)\n", encoding="utf-8")
            with patch("worker_transport.BRIDGE_PATH", script):
                with self.assertRaisesRegex(ValueError, r"exited during startup \(19\).*fixture-startup-diagnostic"):
                    ensure_daemon(root, True)
            with FileLock(root / "startup.guard", timeout=0):
                pass


if __name__ == "__main__":
    unittest.main()
