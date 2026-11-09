import copy
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
import zipfile
from release import ROOT, package, sha256, validate_image, version


class ReleaseTests(unittest.TestCase):
    def test_online_package_has_only_declared_files_and_matching_hashes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "fixture.exe"
            binary.write_bytes(b"not executed")
            revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
            image = dict(repository="hlmark0/re-agent-workbench", digest="sha256:" + "a" * 64,
                         image_id="sha256:" + "b" * 64, source_revision=revision, platform="linux/amd64", size_bytes=10)
            metadata = root / "image.json"
            metadata.write_text(json.dumps(image))
            output = root / "dist"
            manifest = package(binary, metadata, output, "v" + version())
            self.assertEqual(manifest["files"]["re-agent.exe"], sha256(binary))
            with zipfile.ZipFile(next(output.glob("*.zip"))) as archive:
                self.assertEqual(set(archive.namelist()), set(manifest["files"]) | {"release.json"})
                self.assertEqual(json.loads(archive.read("release.json")), manifest)

    def test_repository_versions_agree(self):
        self.assertEqual(version("v" + version()), version())

    def test_mismatched_tag_is_rejected(self):
        with self.assertRaises(ValueError):
            version("v999.0.0")

    def test_image_identity_and_platform(self):
        data = dict(repository="hlmark0/re-agent-workbench", digest="sha256:" + "a" * 64,
                    image_id="sha256:" + "b" * 64, source_revision="abc", platform="linux/amd64", size_bytes=10)
        validate_image(data, "abc")
        for field, value in [("source_revision", "other"), ("platform", "linux/arm64"),
                             ("digest", "latest"), ("image_id", "wrong"),
                             ("repository", "other.example/x"), ("size_bytes", -1)]:
            changed = copy.copy(data)
            changed[field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                validate_image(changed, "abc")


if __name__ == "__main__":
    unittest.main()
