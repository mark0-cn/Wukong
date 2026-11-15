"""Exercise real test-relay pipes, including a relay that never reads input."""

from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

from support.client import Client


class ClientDeadlineTests(unittest.TestCase):
    def client(self, directory, program):
        launch = subprocess.Popen
        with patch(
            "support.client.subprocess.Popen",
            side_effect=lambda _command, **options: launch(
                [sys.executable, "-u", "-c", program], **options
            ),
        ):
            return Client(Path(directory))

    def test_blocked_write_times_out_and_reaps_only_the_relay(self):
        with tempfile.TemporaryDirectory() as directory:
            client = self.client(directory, "import time; time.sleep(60)")
            started = time.monotonic()
            try:
                with self.assertRaisesRegex(AssertionError, "test RPC file.put reply 1 exceeded"):
                    client.call("file.put", data_base64="x" * 1048576, rpc_timeout=0.2)
                self.assertLess(time.monotonic() - started, 10)
                self.assertIsNotNone(client.process.poll())
                self.assertFalse(client.exchange.is_alive())
                self.assertTrue(client.process.stdin.closed)
                with self.assertRaisesRegex(AssertionError, "reconnect explicitly"):
                    client.call("ping")
            finally:
                client.close()

    def test_finish_deadline_includes_waiting_for_status_reply(self):
        with tempfile.TemporaryDirectory() as directory:
            client = self.client(directory, "import sys,time; sys.stdin.readline(); time.sleep(60)")
            try:
                with self.assertRaisesRegex(AssertionError, "test RPC job.status reply 1 exceeded"):
                    client.finish({"job_id": "fixture"}, timeout=0.2)
                self.assertIsNotNone(client.process.poll())
            finally:
                client.close()

    def test_mismatched_response_is_not_consumed_by_another_call(self):
        with tempfile.TemporaryDirectory() as directory:
            client = self.client(
                directory,
                'import sys; sys.stdin.readline(); print(\'{"id":99,"result":{}}\', flush=True)',
            )
            try:
                with self.assertRaisesRegex(AssertionError, "expected reply 1, got 99"):
                    client.call("ping", rpc_timeout=5)
                self.assertTrue(client.closed)
            finally:
                client.close()
