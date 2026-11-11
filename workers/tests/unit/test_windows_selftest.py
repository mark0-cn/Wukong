import hashlib
from pathlib import Path
import tempfile
import unittest

from worker_selftest import checked_file


class VerifiedSelftestMembers(unittest.TestCase):
    def test_probe_hashes_and_paths_are_checked_before_execution(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "probe.exe").write_bytes(b"known fixture")
            manifest = {"files": {"probe.exe": {"sha256": hashlib.sha256(b"known fixture").hexdigest(), "size": 13}}}
            self.assertEqual(checked_file(root, manifest, "probe.exe"), root / "probe.exe")
            (root / "probe.exe").write_bytes(b"changed bytes")
            with self.assertRaises(ValueError):
                checked_file(root, manifest, "probe.exe")
            for name in ["../probe.exe", "/probe.exe", "runtime\\probe.exe", "runtime//probe.exe"]:
                with self.subTest(name=name), self.assertRaises(ValueError):
                    checked_file(root, manifest, name)


if __name__ == "__main__":
    unittest.main()
