import copy
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from scripts.worker.assemble import BINARIES, file_index, native_build
from tests.automation.test_worker_pe import fixture


class AssemblyChecks(unittest.TestCase):
    def test_native_provenance_refuses_dirty_stale_or_changed_binaries(self):
        source = dict(source_revision="a" * 40, source_tree_sha256="b" * 64, source_dirty=False)
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binaries = root / "x86_64-pc-windows-msvc/release"
            binaries.mkdir(parents=True)
            files = {}
            for name in BINARIES:
                payload = fixture("KERNEL32.dll")
                (binaries / name).write_bytes(payload)
                files[name] = dict(sha256=hashlib.sha256(payload).hexdigest(), size=len(payload), imported_libraries=["kernel32.dll"])
            original = dict(schema=1, platform="windows-amd64", static_crt_checked=True, files=files, **source)
            metadata = root / "native-build.json"
            for change in [{}, dict(source_dirty=True), dict(source_revision="c" * 40), dict(source_tree_sha256="d" * 64)]:
                value = copy.deepcopy(original)
                value.update(change)
                metadata.write_text(json.dumps(value))
                with patch("scripts.worker.assemble.identity", return_value=source):
                    if change:
                        with self.assertRaises(ValueError):
                            native_build(root)
                    else:
                        self.assertEqual(native_build(root)[0], binaries)
            metadata.write_text(json.dumps(original))
            (binaries / BINARIES[0]).write_bytes(fixture("ADVAPI32.dll"))
            with patch("scripts.worker.assemble.identity", return_value=source), self.assertRaises(ValueError):
                native_build(root)

    def test_manifest_index_assigns_originals_licenses_sources_and_runtime(self):
        entries = {"originals/payload.zip": "original_installer", "licenses/native/LICENSE": "license",
                   "runtime/python/LICENSE.txt": "license", "runtime/worker/bridge.py": "worker_source",
                   "runtime/python/Lib/site-packages/example.dist-info/LICENSE": "license",
                   "runtime/native/wukong-worker.exe": "runtime", "native-build.json": "data"}
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for name in entries:
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(b"fixture")
            actual = file_index(root)
            self.assertEqual({name: item["kind"] for name, item in actual.items()}, entries)
            self.assertTrue(all(item["sha256"] == hashlib.sha256(b"fixture").hexdigest() for item in actual.values()))


if __name__ == "__main__":
    unittest.main()
