"""Protocol definitions participate in the signed native build identity."""
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from scripts.worker import source_identity


class WorkerSourceIdentityTests(unittest.TestCase):
    def test_protocol_edit_changes_source_hash(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            schema = root / "workers/windows/proto/broker.proto"
            schema.parent.mkdir(parents=True)
            schema.write_text('syntax = "proto3"; message Request {}', encoding="utf-8")
            with patch.object(source_identity, "ROOT", root), patch.object(
                source_identity.subprocess, "check_output", return_value="a" * 40
            ):
                before = source_identity.identity()["source_tree_sha256"]
                schema.write_text('syntax = "proto3"; message Request { uint32 schema = 1; }', encoding="utf-8")
                after = source_identity.identity()["source_tree_sha256"]
            self.assertNotEqual(before, after)
