import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from scripts._lib.versioning import ROOT, version
from scripts.release import local_release


def source_constant(path, name):
    text = (ROOT / path).read_text(encoding="utf-8")
    return int(re.search(rf"pub const {name}: u32 = ([0-9]+);", text).group(1))


class DescriptorTests(unittest.TestCase):
    def test_descriptor_records_the_current_sources(self):
        with (
            patch.dict(os.environ, {}, clear=False),
            patch.object(local_release, "image_identity", return_value=(1024, "linux/amd64")),
            patch.object(local_release, "template", return_value=None),
        ):
            payload = local_release.descriptor()
        self.assertEqual(payload["schema"], 1)
        self.assertEqual(payload["version"], version())
        self.assertEqual(
            payload["runtime_protocol"],
            source_constant(local_release.PROTOCOL_LIB, "PROTOCOL"),
        )
        self.assertEqual(
            payload["data_schema"],
            source_constant(local_release.DATA_SCHEMA_MANIFEST, "DATA_SCHEMA"),
        )
        self.assertEqual(payload["image"], local_release.DEFAULT_IMAGE)
        self.assertEqual(payload["image_size"], 1024)
        self.assertRegex(payload["source_revision"], r"^[0-9a-f]{40}$")
        self.assertRegex(payload["built_at"], r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")
        self.assertNotIn("template_path", payload)

    def test_a_packaged_template_is_recorded_as_an_absolute_path(self):
        with (
            tempfile.TemporaryDirectory() as tmp,
            patch.object(local_release, "image_identity", return_value=(1024, "linux/amd64")),
            patch.object(
                local_release, "template", return_value=Path(tmp).resolve() / "template.zip"
            ),
        ):
            payload = local_release.descriptor()
        self.assertEqual(payload["template_path"], str(Path(tmp).resolve() / "template.zip"))

    def test_image_override_comes_from_the_environment(self):
        with patch.dict(os.environ, {local_release.IMAGE_ENV: "wukong:test"}, clear=False):
            self.assertEqual(local_release.image(), "wukong:test")
        with patch.dict(os.environ, {local_release.IMAGE_ENV: "-bad tag"}, clear=False):
            with self.assertRaisesRegex(ValueError, "Invalid local Docker image tag"):
                local_release.image()

    def test_missing_image_is_built_from_the_bake_target(self):
        completed = subprocess.CompletedProcess([], 0, "", "")
        with (
            patch.object(local_release, "_image_field", side_effect=[None, "sha256:abc"]) as field,
            patch.object(local_release.subprocess, "run", return_value=completed) as run,
        ):
            local_release.ensure_image("wukong:local")
        field.assert_any_call("wukong:local", "{{.Id}}")
        command = run.call_args.args[0]
        self.assertEqual(command[:4], ["docker", "buildx", "bake", "--load"])
        self.assertIn("workbench.tags=wukong:local", command)

    def test_a_failed_bake_reports_the_actionable_command(self):
        with (
            patch.object(local_release, "_image_field", return_value=None),
            patch.object(
                local_release.subprocess,
                "run",
                side_effect=subprocess.CalledProcessError(1, ["docker"]),
            ),
        ):
            with self.assertRaisesRegex(ValueError, "fetch_worker_deps|buildx bake"):
                local_release.ensure_image("wukong:local")

    def test_non_linux_images_are_refused(self):
        with (
            patch.object(
                local_release, "_image_field", side_effect=["sha256:abc", "windows/amd64"]
            ),
        ):
            with self.assertRaisesRegex(ValueError, "linux/amd64"):
                local_release.image_identity("wukong:local")

    def test_image_size_must_be_positive(self):
        with (
            patch.object(
                local_release, "_image_field", side_effect=["sha256:abc", "linux/amd64", "0"]
            ),
        ):
            with self.assertRaisesRegex(ValueError, "reports no size"):
                local_release.image_identity("wukong:local")

    def test_template_environment_must_name_an_existing_zip(self):
        with tempfile.TemporaryDirectory() as tmp:
            missing = Path(tmp) / "missing.zip"
            with patch.dict(os.environ, {local_release.TEMPLATE_ENV: str(missing)}, clear=False):
                with self.assertRaisesRegex(ValueError, "does not name an existing file"):
                    local_release.template()
            invalid = Path(tmp) / "invalid.zip"
            invalid.write_bytes(b"not a zip")
            with patch.dict(os.environ, {local_release.TEMPLATE_ENV: str(invalid)}, clear=False):
                with self.assertRaisesRegex(ValueError, "not a ZIP archive"):
                    local_release.template()

    def test_prepare_writes_the_bundled_resource_shape(self):
        payload = {
            "schema": 1,
            "version": version(),
            "runtime_protocol": 3,
            "data_schema": 5,
            "image": "wukong:local",
            "image_size": 1024,
            "built_at": "2026-11-18T00:00:00Z",
            "source_revision": "a" * 40,
        }
        with (
            tempfile.TemporaryDirectory() as tmp,
            patch.object(local_release, "descriptor", return_value=payload),
        ):
            target, written = local_release.prepare(Path(tmp) / "local-release.json")
            self.assertEqual(json.loads(target.read_text(encoding="utf-8")), payload)
        self.assertEqual(written, payload)

    def test_descriptor_version_comes_from_cargo_toml(self):
        with (
            patch.object(local_release, "image_identity", return_value=(1024, "linux/amd64")),
            patch.object(local_release, "template", return_value=None),
        ):
            self.assertNotEqual(local_release.descriptor()["version"], "development")


if __name__ == "__main__":
    unittest.main()
