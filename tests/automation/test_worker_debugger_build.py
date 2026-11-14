"""A patched debugger must retain its source, binary and license identity."""
import hashlib
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from scripts.worker import build_debugger


class DebuggerBuildChecks(unittest.TestCase):
    def test_refuses_changed_source_patch_binary_or_license(self):
        provenance = dict(upstream_revision="a" * 40, source_sha256="b" * 64, patch_sha256="c" * 64)
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            files = {}
            for name in [build_debugger.BINARY, build_debugger.PATCH_MEMBER, "debugger/licenses/LICENSE"]:
                target = root / name
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(b"fixture")
                files[name] = dict(size=7, sha256=hashlib.sha256(b"fixture").hexdigest())
            record = dict(**provenance, files=files)
            with patch.object(build_debugger, "inputs", return_value=({}, provenance)), patch.dict(
                build_debugger.SUPPLEMENTAL_LICENSES, {}, clear=True
            ):
                build_debugger.validate(root, record)
                for key in provenance:
                    with self.subTest(key=key), self.assertRaisesRegex(ValueError, "pinned source"):
                        build_debugger.validate(root, dict(record, **{key: "d" * 64}))
                for name in files:
                    (root / name).write_bytes(b"changed")
                    with self.subTest(name=name), self.assertRaisesRegex(ValueError, "artifact changed"):
                        build_debugger.validate(root, record)
                    (root / name).write_bytes(b"fixture")
