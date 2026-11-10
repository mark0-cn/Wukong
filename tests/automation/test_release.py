import copy
import json
from pathlib import Path
import subprocess
import tempfile
import tomllib
import unittest
from unittest.mock import patch
import zipfile
from scripts._lib.versioning import ROOT, VERSION_FILES, bump_version, sha256, validate_tag, version
from scripts.release.cli import package, validate_image


class ReleaseTests(unittest.TestCase):
    def version_fixture(self, root, crlf=False):
        for name in (*VERSION_FILES, "rust-toolchain.toml"):
            destination = root / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            raw = (ROOT / name).read_bytes().replace(b"\r\n", b"\n")
            destination.write_bytes(raw.replace(b"\n", b"\r\n") if crlf else raw)

    def test_bump_updates_all_versions_without_dependency_changes(self):
        for crlf in (False, True):
            with self.subTest(crlf=crlf), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                self.version_fixture(root, crlf)
                before = {name: (root / name).read_bytes() for name in VERSION_FILES}
                old = version(root=root)
                target = f"{int(old.split('.')[0]) + 1}.10.0"
                self.assertEqual(bump_version("v" + target, root), target)
                self.assertEqual(version("v" + target, root), target)
                for name in VERSION_FILES:
                    if name.endswith(".json"):
                        old_data = json.loads(before[name])
                        new_data = json.loads((root / name).read_bytes())
                        if name == "package-lock.json":
                            new_data["packages"]["ui"]["version"] = old
                        else:
                            new_data["version"] = old
                    else:
                        old_data = tomllib.loads(before[name].decode())
                        new_data = tomllib.loads((root / name).read_text())
                        if name == "Cargo.lock":
                            next(p for p in new_data["package"] if p["name"] == "wukong")["version"] = old
                        else:
                            new_data["package"]["version"] = old
                    self.assertEqual(new_data, old_data, name)
                    if crlf:
                        self.assertNotIn(b"\n", (root / name).read_bytes().replace(b"\r\n", b""))

    def test_bump_rejects_equal_lower_and_invalid_versions_without_writes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.version_fixture(root)
            before = {name: (root / name).read_bytes() for name in VERSION_FILES}
            for tag in ["v" + version(root=root), "v0.0.0", "v01.2.3", "1.2.3", "v1.2.3-rc1"]:
                with self.subTest(tag=tag), self.assertRaises(ValueError):
                    bump_version(tag, root)
                self.assertEqual({name: (root / name).read_bytes() for name in VERSION_FILES}, before)

    def test_bump_refuses_inconsistent_source(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.version_fixture(root)
            config = json.loads((root / "tauri.conf.json").read_bytes())
            config["version"] = "999.0.0"
            (root / "tauri.conf.json").write_text(json.dumps(config), encoding="utf-8")
            before = {name: (root / name).read_bytes() for name in VERSION_FILES}
            with self.assertRaisesRegex(ValueError, "Versions differ"):
                bump_version("v9999.0.0", root)
            self.assertEqual({name: (root / name).read_bytes() for name in VERSION_FILES}, before)

    def test_bump_restores_files_when_postcheck_fails(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.version_fixture(root)
            before = {name: (root / name).read_bytes() for name in VERSION_FILES}
            with patch("scripts._lib.versioning.version", side_effect=[version(root=root), ValueError("postcheck")]):
                with self.assertRaisesRegex(ValueError, "postcheck"):
                    bump_version("v9999.0.0", root)
            self.assertEqual({name: (root / name).read_bytes() for name in VERSION_FILES}, before)

    def test_online_package_has_only_declared_files_and_matching_hashes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "fixture.exe"
            binary.write_bytes(b"not executed")
            revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
            image = dict(repository="hlmark0/wukong", digest="sha256:" + "a" * 64,
                         image_id="sha256:" + "b" * 64, source_revision=revision, platform="linux/amd64", size_bytes=10)
            metadata = root / "image.json"
            metadata.write_text(json.dumps(image))
            output = root / "dist"
            manifest = package(binary, metadata, output, "v" + version())
            self.assertEqual(manifest["files"]["wukong.exe"], sha256(binary))
            with zipfile.ZipFile(next(output.glob("*.zip"))) as archive:
                self.assertEqual(set(archive.namelist()), set(manifest["files"]) | {"release.json"})
                self.assertEqual(json.loads(archive.read("release.json")), manifest)

    def test_repository_versions_agree(self):
        self.assertEqual(version("v" + version()), version())

    def test_mismatched_tag_is_rejected(self):
        with self.assertRaises(ValueError):
            version("v999.0.0")

    def test_release_tag_accepts_three_numeric_components(self):
        for tag in ["v0.0.0", "v0.1.0", "v1.2.3", "v12.34.567"]:
            with self.subTest(tag=tag):
                self.assertEqual(validate_tag(tag), tag[1:])

    def test_invalid_tag_is_rejected_before_repository_files_are_read(self):
        for tag in ["", "1.2.3", "V1.2.3", "v1", "v1.2", "v1.2.3.4", "v01.2.3", "v1.02.3",
                    "v1.2.03", "v1.2.3-rc.1", "v1.2.3+build", "vlatest", "release/v1.2.3",
                    "v1.2.3\n", " v1.2.3", "v1.2.٣", "v-1.2.3"]:
            with self.subTest(tag=tag), self.assertRaisesRegex(ValueError, "Invalid release tag"):
                version(tag, root=Path("nonexistent-repository"))

    def test_image_identity_and_platform(self):
        data = dict(repository="hlmark0/wukong", digest="sha256:" + "a" * 64,
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
