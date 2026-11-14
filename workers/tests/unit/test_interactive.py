"""Retain final process output when leader exit precedes stdout draining."""
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import sys
import tempfile
import threading
import unittest
from unittest.mock import patch

from worker_interactive import InteractSession


class InteractiveDrainTests(unittest.TestCase):
    def test_exited_leader_waits_for_stdout_eof_within_the_read_deadline(self):
        original = InteractSession.drain
        release = threading.Event()
        finished = threading.Event()
        def delayed(session):
            try:
                release.wait(5)
                original(session)
            finally:
                finished.set()
        with tempfile.TemporaryDirectory() as temporary, patch.object(InteractSession, "drain", delayed):
            session = InteractSession(Path(temporary), {"argv": [sys.executable, "-c", "print('final output', flush=True)"]})
            try:
                self.assertEqual(session.process.wait(timeout=5), 0)
                waiting = session.write(b"", 0.05)
                self.assertTrue(waiting["timed_out"], "leader exit does not establish stdout EOF")
                self.assertFalse(waiting["data_sent"])
                with ThreadPoolExecutor(max_workers=1) as executor:
                    result = executor.submit(session.write, b"", 2)
                    release.set()
                    settled = result.result(timeout=3)
                self.assertIn("final output", settled["captured"])
                self.assertFalse(settled["timed_out"])
                self.assertFalse(settled["data_sent"])
            finally:
                release.set()
                session.close()
                self.assertTrue(finished.wait(3), "fixture reader must exit before temporary cleanup")
