import copy
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from scripts.worker.assemble import BINARIES, SUPPLEMENTAL_LICENSES, file_index, native_build
from scripts.worker.notices import collect
from scripts._lib.checksums import sha256
from scripts._lib.repository import ROOT
from tests.automation.test_worker_pe import fixture


class AssemblyChecks(unittest.TestCase):
    def test_native_supplement_preserves_exact_upstream_license_and_version(self):
        original = SUPPLEMENTAL_LICENSES[("fixture3-ddmin", "0.1.1")]
        provenance = json.loads(original.with_name("source.json").read_text(encoding="utf-8"))
        self.assertEqual(sha256(original), provenance["sha256"])
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            dependency = root / "dependency"
            dependency.mkdir()
            sysroot = root / "rust"
            library = sysroot / "share/doc/rust/COPYRIGHT-library.html"
            library.parent.mkdir(parents=True)
            library.write_bytes(b"<p>Rust fixture notice</p>")
            package = dict(name="fixture3-ddmin", version="0.1.1", source="registry+fixture",
                           license="MIT OR Apache-2.0", manifest_path=str(dependency / "Cargo.toml"))
            with patch("scripts.worker.notices.subprocess.check_output", side_effect=[
                json.dumps(dict(packages=[package])).encode(), str(sysroot), "rustc fixture",
            ]):
                paths = collect(root / "notices", supplemental=SUPPLEMENTAL_LICENSES)
            copied = root / "notices/fixture3-ddmin-0.1.1/LICENSE"
            self.assertIn(copied, paths)
            self.assertEqual(copied.read_bytes(), original.read_bytes())
            package["version"] = "0.1.2"
            with patch("scripts.worker.notices.subprocess.check_output",
                       return_value=json.dumps(dict(packages=[package])).encode()):
                with self.assertRaisesRegex(ValueError, "Missing native dependency license"):
                    collect(root / "different-version", supplemental=SUPPLEMENTAL_LICENSES)

    def test_native_notices_preserve_license_text_shipped_as_copyright(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            dependency = root / "dependency"
            dependency.mkdir()
            original = b"MIT fixture license\r\nCopyright (c) fixture author\r\n"
            (dependency / "COPYRIGHT").write_bytes(original)
            sysroot = root / "rust"
            library = sysroot / "share/doc/rust/COPYRIGHT-library.html"
            library.parent.mkdir(parents=True)
            library.write_bytes(b"<p>Rust fixture notice</p>")
            package = dict(name="fixture", version="1.2.3", source="registry+fixture", license="MIT",
                           manifest_path=str(dependency / "Cargo.toml"))
            output = root / "notices"
            with patch("scripts.worker.notices.subprocess.check_output", side_effect=[
                json.dumps(dict(packages=[package])).encode(), str(sysroot), "rustc fixture",
            ]) as commands:
                paths = collect(output)
            command = commands.call_args_list[0].args[0]
            self.assertIn("--offline", command)
            self.assertIn("--locked", command)
            copied = output / "fixture-1.2.3/COPYRIGHT"
            self.assertIn(copied, paths)
            self.assertEqual(copied.read_bytes(), original)
            record = json.loads((output / "native-notices.json").read_text(encoding="utf-8"))
            self.assertEqual(record["packages"], [dict(name="fixture", version="1.2.3", license="MIT",
                                                    files=["fixture-1.2.3/COPYRIGHT"])])

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
            probes = {}
            for architecture in ["x64", "x86"]:
                binary = f"selftest/{architecture}/wukong-probe.exe"
                pdb = f"selftest/{architecture}/wukong-probe.pdb"
                (root / binary).parent.mkdir(parents=True)
                (root / binary).write_bytes(fixture("KERNEL32.dll", architecture=architecture))
                (root / pdb).write_bytes(b"fixture pdb")
                probes[architecture] = dict(binary=binary, pdb=pdb, binary_sha256=sha256(root / binary), pdb_sha256=sha256(root / pdb),
                    imported_libraries=["kernel32.dll"], source_sha256=sha256(ROOT / "workers/windows/fixtures/probe.c"))
            original = dict(schema=1, platform="windows-amd64", static_crt_checked=True, files=files,
                            selftests=dict(schema=1, architectures=probes), **source)
            metadata = root / "native-build.json"
            for change in [{}, dict(source_dirty=True), dict(source_revision="c" * 40), dict(source_tree_sha256="d" * 64), dict(selftests={})]:
                value = copy.deepcopy(original)
                value.update(change)
                metadata.write_text(json.dumps(value))
                with patch("scripts.worker.assemble.identity", return_value=source), \
                        patch("scripts.worker.assemble.require_static_crt", return_value=["kernel32.dll"]):
                    if change:
                        with self.assertRaises(ValueError):
                            native_build(root)
                    else:
                        self.assertEqual(native_build(root)[0], binaries)
            metadata.write_text(json.dumps(original))
            (binaries / BINARIES[0]).write_bytes(fixture("ADVAPI32.dll"))
            with patch("scripts.worker.assemble.identity", return_value=source), self.assertRaises(ValueError):
                native_build(root)
            (binaries / BINARIES[0]).write_bytes(fixture("KERNEL32.dll"))
            (binaries / "wukong-package.exe").write_bytes(b"changed inspector")
            with patch("scripts.worker.assemble.identity", return_value=source), \
                    patch("scripts.worker.assemble.require_static_crt") as inspect, self.assertRaises(ValueError):
                native_build(root)
            inspect.assert_not_called()

    def test_manifest_index_assigns_originals_licenses_sources_and_runtime(self):
        entries = {"originals/payload.zip": "original_installer", "licenses/native/LICENSE": "license",
                   "runtime/python/LICENSE.txt": "license", "runtime/helpers/selftest.py": "python_source",
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
