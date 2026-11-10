import hashlib
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import scripts.toolchain.fetch_worker_deps as deps


class DependencyTests(unittest.TestCase):
    def test_cache_hit_is_verified_and_corrupt_download_does_not_replace_file(self):
        entry = dict(file="tool.zip", url="https://example.invalid/tool", sha256=hashlib.sha256(b"valid").hexdigest())
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp)
            path = folder / "tool.zip"
            path.write_bytes(b"valid")
            with patch.object(deps.subprocess, "run") as download:
                deps.fetch(("tool", entry), folder)
                download.assert_not_called()
            path.write_bytes(b"old")
            def corrupt(*args, **kwargs):
                (folder / "tool.zip.download").write_bytes(b"corrupt")
            with patch.object(deps.subprocess, "run", side_effect=corrupt), self.assertRaises(ValueError):
                deps.fetch(("tool", entry), folder)
            self.assertEqual(path.read_bytes(), b"old")
            self.assertFalse((folder / "tool.zip.download").exists())

    def test_parallel_outputs_cannot_collide_or_escape_directory(self):
        entry = dict(file="tool", url="https://example.invalid/tool", sha256="a" * 64)
        for filename in ("../tool", "/tmp/tool", "C:\\tool", "", "tool/other"):
            with self.assertRaises(ValueError):
                deps.entries(dict(tool=dict(entry, file=filename)))
        with self.assertRaises(ValueError):
            deps.entries(dict(first=entry, second=dict(entry, file="TOOL")))
