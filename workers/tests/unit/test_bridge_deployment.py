"""Signed runtime sources must remain unchanged across bridge process startup."""
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


class ImmutableDeployment(unittest.TestCase):
    def test_bridge_entry_does_not_generate_unsigned_bytecode(self):
        source = Path(__file__).resolve().parents[2] / "runtime"
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for path in source.glob("*.py"):
                shutil.copyfile(path, root / path.name)
            before = {path.name: path.read_bytes() for path in root.iterdir()}
            # Do not inherit a caller's PYTHONDONTWRITEBYTECODE: the production
            # entry itself must protect daemon and job-supervisor interpreters.
            result = subprocess.run([sys.executable, "-E", str(root / "bridge.py"), "--help"],
                                    capture_output=True, text=True, timeout=20)
            self.assertEqual(result.returncode, 0, result.stderr)
            after = {path.relative_to(root).as_posix(): path.read_bytes() for path in root.rglob("*") if path.is_file()}
            self.assertEqual(before, after)


if __name__ == "__main__":
    unittest.main()
