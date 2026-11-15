"""Recovery keeps original damaged bytes and never adopts a dead session owner."""

import json
from pathlib import Path
import tempfile
import unittest
from wukong_worker.resources.sessions import SessionManager


class SessionRecoveryTests(unittest.TestCase):
    def test_unreadable_metadata_is_preserved_and_diagnostics_are_bounded(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            damaged = []
            for index in range(20):
                path = root / "debug" / f"{index:032x}" / "session.json"
                path.parent.mkdir(parents=True)
                path.write_bytes(b'{"secret-input": "DO_NOT_LOG_THIS"')
                damaged.append(path)
            running = root / "interact" / ("a" * 32) / "session.json"
            running.parent.mkdir(parents=True)
            running.write_text('{"state":"running"}', encoding="utf-8")
            terminal = root / "interact" / ("b" * 32) / "session.json"
            terminal.parent.mkdir(parents=True)
            original = b'{"state": "closed", "evidence": "unchanged"}\n'
            terminal.write_bytes(original)
            with self.assertLogs("wukong_worker.resources.sessions", level="WARNING") as logs:
                manager = SessionManager(root, True, False)
            self.assertEqual(len(logs.output), 17)
            self.assertNotIn("DO_NOT_LOG_THIS", "\n".join(logs.output))
            self.assertNotIn(str(root), "\n".join(logs.output))
            self.assertEqual(json.loads(running.read_text())["state"], "recovery_required")
            self.assertFalse(json.loads(running.read_text())["cleanup_confirmed"])
            self.assertEqual(terminal.read_bytes(), original)
            self.assertEqual(manager.debug, {})
            self.assertEqual(manager.interact, {})
            for path in damaged:
                self.assertEqual(path.read_bytes(), b'{"secret-input": "DO_NOT_LOG_THIS"')

    def test_missing_state_remains_a_startup_error(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / "debug" / ("a" * 32) / "session.json"
            path.parent.mkdir(parents=True)
            path.write_text('{"unrecognized":true}', encoding="utf-8")
            with self.assertRaises(KeyError):
                SessionManager(root, True, False)
