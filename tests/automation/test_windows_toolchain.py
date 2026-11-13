import copy
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import zipfile

from scripts.worker.layout import unpack_zip
from scripts.worker.msi_layout import extract_cabinet, plan
from scripts.worker.paths import member, no_links, unique_paths
from scripts.worker.toolchain import acquire, load_lock


class OfflineAssets(unittest.TestCase):
    def test_pinned_inventory_covers_full_sdk_and_runtime_requirements(self):
        lock = load_lock()
        self.assertEqual(len(lock["assets"]), 49)
        sdk = [item for item in lock["assets"] if item["path"].startswith("sdk/")]
        self.assertEqual(len(sdk), 43)
        self.assertEqual(sum(item["path"].endswith(".cab") for item in sdk), 40)
        self.assertTrue(all(len(item["vendor_sha1"]) == 40 for item in sdk))
        self.assertEqual(lock["components"]["openssh"]["channel"], "preview")
        requirements = Path("workers/runtime/requirements.txt").read_text().split()
        pinned = lock["components"]["worker_python_dependencies"]["version"].split(";")
        self.assertEqual(sorted(requirements), sorted(value.replace("=", "==") for value in pinned))

    def test_cache_requires_exact_bytes_and_never_downloads_implicitly(self):
        payload = b"verified official payload fixture"
        item = dict(path="python.zip", size=len(payload), sha256=hashlib.sha256(payload).hexdigest(),
                    url="https://www.python.org/fixture.zip")
        lock = dict(assets=[item])
        with tempfile.TemporaryDirectory() as temporary, patch("urllib.request.build_opener") as opener:
            root = Path(temporary)
            with self.assertRaisesRegex(ValueError, "Missing offline"):
                acquire(root, lock)
            opener.assert_not_called()
            (root / item["path"]).write_bytes(payload)
            self.assertEqual(acquire(root, lock)["assets"], 1)
            opener.assert_not_called()
            (root / item["path"]).write_bytes(b"x" * len(payload))
            with self.assertRaisesRegex(ValueError, "mismatch"):
                acquire(root, lock, download=True)
            opener.assert_not_called()

    def test_download_is_bounded_and_only_publishes_verified_bytes(self):
        payload = b"pinned fixture"
        item = dict(path="payload.zip", size=len(payload), sha256=hashlib.sha256(payload).hexdigest(),
                    url="https://www.python.org/fixture.zip")
        for actual in [b"truncated", b"x" * len(payload), payload + b"extra", payload]:
            with self.subTest(actual=actual), tempfile.TemporaryDirectory() as temporary:
                with patch("urllib.request.build_opener") as opener:
                    opener.return_value.open.return_value = io.BytesIO(actual)
                    if actual == payload:
                        acquire(temporary, dict(assets=[item]), download=True)
                        self.assertEqual((Path(temporary) / item["path"]).read_bytes(), payload)
                    else:
                        with self.assertRaises(ValueError):
                            acquire(temporary, dict(assets=[item]), download=True)
                        self.assertEqual(list(Path(temporary).iterdir()), [])

    def test_lock_rejects_insecure_origins_and_aliases(self):
        original = load_lock()
        for changed in [dict(url="http://www.python.org/file.zip"), dict(url="https://www.python.org@evil.test/file"),
                        dict(path="CON.zip"), dict(path="../payload"), dict(size=True), dict(sha256="a" * 63)]:
            lock = copy.deepcopy(original)
            lock["assets"][0].update(changed)
            with self.subTest(changed=changed), tempfile.TemporaryDirectory() as temporary:
                path = Path(temporary) / "lock.json"
                path.write_text(json.dumps(lock))
                with self.assertRaises(ValueError):
                    load_lock(path)


class OfflinePaths(unittest.TestCase):
    def test_ambiguous_windows_names_and_file_directory_collisions_are_rejected(self):
        for name in ["../x", "C:/x", "x\\y", "/x", "x/", "x:", "COM0.txt", "lpt9", "con.zip",
                     "foo.", " foo", "foo .txt", "foo ", "x//y", "x/..", "非ASCII"]:
            with self.subTest(name=name), self.assertRaises(ValueError):
                member(name)
        for names in [["A.txt", "a.TXT"], ["file", "FILE/child"]]:
            with self.assertRaises(ValueError):
                unique_paths(names)
        self.assertEqual(member("Windows Kits/10/Debuggers/x64/dbgeng.dll"), "Windows Kits/10/Debuggers/x64/dbgeng.dll")

    def test_linked_ancestors_are_rejected_before_resolution(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "real").mkdir()
            if os.name == "nt":
                result = subprocess.run(["cmd.exe", "/d", "/c", "mklink", "/J", str(root / "link"), str(root / "real")],
                                        capture_output=True, text=True, check=True)
                self.assertEqual(result.returncode, 0)
            else:
                (root / "link").symlink_to(root / "real", target_is_directory=True)
            with self.assertRaisesRegex(ValueError, "Linked"):
                no_links(root / "link/new-file")


class MsiPlan(unittest.TestCase):
    def fixture(self):
        return {"Directory": [["TARGETDIR", "", "SourceDir"], ["base", "TARGETDIR", "SHORT|Tools:Source"],
                              ["inherit", "base", "."]], "Component": [["component", "inherit"]],
                "File": [["file_id", "component", "LONG~1.DLL|long.dll", "4", "2"],
                         ["plain", "component", "original.msi", "8", "1"]],
                "Media": [["1", ""], ["2", "payload.cab"]]}

    def test_msi_long_names_source_names_and_media_ranges(self):
        files = plan(self.fixture())
        self.assertEqual([(f["path"], f["cabinet"]) for f in files],
                         [("Tools/long.dll", "payload.cab"), ("Tools/original.msi", "")])

    def test_rejects_cycles_unknown_media_duplicate_files_and_unsafe_targets(self):
        for change in [lambda t: t["Directory"][1].__setitem__(1, "inherit"),
                       lambda t: t["File"][0].__setitem__(4, "3"),
                       lambda t: t["File"][0].__setitem__(2, "../escape.dll"),
                       lambda t: t["File"][0].__setitem__(1, "missing"),
                       lambda t: t["File"].append(t["File"][0]),
                       lambda t: t["Media"][1].__setitem__(1, "#embedded"),
                       lambda t: t["File"][1].__setitem__(2, "LONG.DLL")]:
            tables = self.fixture()
            change(tables)
            with self.assertRaises(ValueError):
                plan(tables)


class OfflineZip(unittest.TestCase):
    def test_rejects_archive_traversal_links_case_aliases_and_overwrite(self):
        for names in [["../outside"], ["A.txt", "a.TXT"], ["Dir/a", "dir/b"], ["file", "file/"]]:
            with self.subTest(names=names), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                archive = root / "test.zip"
                with zipfile.ZipFile(archive, "w") as source:
                    for name in names:
                        source.writestr(name, b"fixture")
                with self.assertRaises(ValueError):
                    unpack_zip(archive, root / "output")
                self.assertFalse((root / "outside").exists())
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = root / "test.zip"
            with zipfile.ZipFile(archive, "w") as source:
                info = zipfile.ZipInfo("link")
                info.external_attr = 0o120777 << 16
                source.writestr(info, "elsewhere")
            with self.assertRaises(ValueError):
                unpack_zip(archive, root / "output")
            with zipfile.ZipFile(archive, "w") as source:
                source.writestr("runtime.dll", b"fixture")
            unpack_zip(archive, root / "output")
            with self.assertRaises(FileExistsError):
                unpack_zip(archive, root / "output")

    def test_wheels_require_pure_metadata_without_unbundled_dependencies(self):
        for requires in ["", "Requires-Dist: missing-runtime\n"]:
            with self.subTest(requires=requires), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                archive = root / "test.whl"
                with zipfile.ZipFile(archive, "w") as source:
                    source.writestr("fixture.py", "print('fixture')")
                    source.writestr("fixture.dist-info/WHEEL", "Root-Is-Purelib: true\nTag: py3-none-any\n")
                    source.writestr("fixture.dist-info/METADATA", "Name: fixture\n" + requires)
                if requires:
                    with self.assertRaisesRegex(ValueError, "dependency closure"):
                        unpack_zip(archive, root / "output", wheel=True)
                else:
                    unpack_zip(archive, root / "output", wheel=True)
                    self.assertTrue((root / "output/fixture.py").is_file())


@unittest.skipUnless(os.name == "nt" and shutil.which("makecab.exe"), "Windows cabinet API acceptance")
class CabinetAcceptance(unittest.TestCase):
    def test_native_cabinet_inventory_and_exact_extraction(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "fixture.txt").write_bytes(b"trusted cabinet fixture")
            subprocess.run([shutil.which("makecab.exe"), str(root / "fixture.txt"), str(root / "fixture.cab")],
                           check=True, capture_output=True)
            for inventory in [{"unexpected.txt": 23}, {"fixture.txt": 999}]:
                with tempfile.TemporaryDirectory(dir=root) as output:
                    with self.assertRaises(ValueError):
                        extract_cabinet(root / "fixture.cab", Path(output) / "rejected", inventory)
            output = root / "accepted"
            extract_cabinet(root / "fixture.cab", output, {"fixture.txt": 23})
            self.assertEqual((output / "fixture.txt").read_bytes(), b"trusted cabinet fixture")


if __name__ == "__main__":
    unittest.main()
