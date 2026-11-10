import contextlib
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import zipfile

from scripts._lib.repository import ROOT
from scripts._lib.versioning import sha256, write_json
from scripts.release import local_package

CHECKOUT_IDENTITY = local_package.checkout_identity


class LocalPackageTests(unittest.TestCase):
    def setUp(self):
        self.stack = contextlib.ExitStack()
        self.addCleanup(self.stack.close)
        self.root = Path(self.stack.enter_context(tempfile.TemporaryDirectory(prefix="local package ")))
        self.revision = "a" * 40
        self.metadata = self.root / "image metadata.json"
        self.output = self.root / "output package"
        self.image = dict(source_revision=self.revision, repository="example/wukong",
                          digest="sha256:" + "b" * 64, image_id="sha256:" + "c" * 64,
                          size_bytes=12345, platform="linux/amd64")
        write_json(self.metadata, self.image)
        for name in ("packaging/windows/install-workbench.ps1", "packaging/windows/rollback-workbench.ps1",
                     "docs/user/online-install.md"):
            destination = self.root / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / name, destination)
        self.stack.enter_context(patch.object(local_package, "ROOT", self.root))
        self.stack.enter_context(patch("scripts.release.cli.ROOT", self.root))
        self.stack.enter_context(patch.object(local_package.platform, "system", return_value="Windows"))
        self.stack.enter_context(patch.object(local_package.platform, "machine", return_value="AMD64"))
        self.stack.enter_context(patch.object(local_package.shutil, "which", side_effect=lambda name: name))
        self.stack.enter_context(patch.object(local_package, "version", return_value="1.2.3"))
        self.stack.enter_context(patch("scripts.release.cli.version", return_value="1.2.3"))
        self.identity = self.stack.enter_context(patch.object(local_package, "checkout_identity", return_value=self.revision))
        self.stack.enter_context(patch("scripts.release.cli.subprocess.check_output", return_value=self.revision))
        self.commands = []
        self.runner = self.stack.enter_context(patch.object(local_package, "run", side_effect=self.fake_run))

    def fake_run(self, command):
        self.commands.append(command)
        if "--locked" in command:
            binary = self.root / "target/x86_64-pc-windows-msvc/release/wukong.exe"
            binary.parent.mkdir(parents=True)
            binary.write_bytes(b"fixture executable; not a real client")

    def test_build_packages_installers_manifest_and_matching_checksums(self):
        local_package.build(self.metadata, self.output)
        self.assertEqual(self.commands[0][1:], ["ci"])
        self.assertEqual(self.commands[1][1:], ["run", "build"])
        self.assertIn("x86_64-pc-windows-msvc", self.commands[2])
        self.assertIn("custom-protocol", self.commands[2])
        archive = self.output / "wukong-v1.2.3-windows-amd64.zip"
        with zipfile.ZipFile(archive) as bundle:
            manifest = json.loads(bundle.read("release.json"))
            self.assertEqual(manifest["source_revision"], self.revision)
            self.assertEqual(manifest["image"], "example/wukong@" + self.image["digest"])
            self.assertEqual(set(bundle.namelist()), {"wukong.exe", "install-workbench.ps1",
                                                     "rollback-workbench.ps1", "README.md", "release.json"})
        self.assertIn(sha256(archive), (self.output / "SHA256SUMS").read_text())

    def test_missing_or_stale_metadata_stops_before_build(self):
        for metadata in (self.root / "missing.json", self.metadata):
            self.image["source_revision"] = "d" * 40
            write_json(self.metadata, self.image)
            with self.subTest(metadata=metadata), self.assertRaises(ValueError):
                local_package.build(metadata, self.output)
        self.runner.assert_not_called()
        self.assertFalse(self.output.exists())

    def test_check_only_does_not_build_or_write(self):
        local_package.build(self.metadata, self.output, check_only=True)
        self.runner.assert_not_called()
        self.assertFalse(self.output.exists())

    def test_default_downloads_matching_release_without_local_metadata(self):
        self.metadata.unlink()
        image = {**self.image, "release_version": "1.2.3", "source_revision": "d" * 40}
        with patch.object(local_package, "release_image", return_value=image) as fetch:
            local_package.build(None, self.output)
        fetch.assert_called_once_with("1.2.3", self.revision, "mark0-cn/Wukong")
        self.assertTrue((self.output / "wukong-v1.2.3-windows-amd64.zip").is_file())
        manifest = json.loads((self.output / "release.json").read_text())
        self.assertEqual(manifest["source_revision"], self.revision)
        self.assertEqual(manifest["image_source_revision"], "d" * 40)
        self.assertEqual(manifest["image_release_version"], "1.2.3")

    def test_released_image_from_different_version_still_stops_before_build(self):
        image = {**self.image, "release_version": "1.2.2", "source_revision": "d" * 40}
        with patch.object(local_package, "release_image", return_value=image):
            with self.assertRaisesRegex(ValueError, "Release version"):
                local_package.build(None, self.output)
        self.runner.assert_not_called()

    def test_release_download_failure_stops_before_compiling(self):
        with patch.object(local_package, "release_image", side_effect=ValueError("Release asset not found")):
            with self.assertRaisesRegex(ValueError, "Release asset not found"):
                local_package.build(None, self.output)
        self.runner.assert_not_called()
        self.assertFalse(self.output.exists())

    def test_explicit_metadata_does_not_access_release(self):
        with patch.object(local_package, "release_image") as fetch:
            local_package.build(self.metadata, self.output, check_only=True)
        fetch.assert_not_called()

    def test_failed_frontend_stops_compilation_and_packaging(self):
        self.runner.side_effect = [None, subprocess.CalledProcessError(7, "npm")]
        with self.assertRaises(subprocess.CalledProcessError):
            local_package.build(self.metadata, self.output)
        self.assertEqual(self.runner.call_count, 2)
        self.assertFalse(self.output.exists())

    def test_existing_destination_is_preserved(self):
        self.output.mkdir()
        marker = self.output / "keep.txt"
        marker.write_text("keep")
        with self.assertRaisesRegex(ValueError, "Output already exists"):
            local_package.build(self.metadata, self.output)
        self.runner.assert_not_called()
        self.assertEqual(marker.read_text(), "keep")

    def test_changed_checkout_during_build_is_not_packaged(self):
        self.identity.side_effect = [self.revision, "d" * 40]
        with self.assertRaisesRegex(ValueError, "Source changed"):
            local_package.build(self.metadata, self.output)
        self.assertFalse(self.output.exists())

    def test_dirty_checkout_is_rejected(self):
        # Exercise the real guard independently of the build orchestration fixture.
        with patch("scripts.release.local_package.subprocess.check_output", side_effect=[self.revision, " M src/main.rs"]):
            with self.assertRaisesRegex(ValueError, "Commit source changes first"):
                CHECKOUT_IDENTITY()


if __name__ == "__main__":
    unittest.main()
