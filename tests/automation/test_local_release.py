import contextlib
import io
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from scripts._lib.protocol import RUNTIME_PROTOCOL
from scripts._lib.versioning import ROOT, version
from scripts.release import local_release

DEFAULT_REGISTRY = "hlmark0/wukong-workbench"


def source_constant(path, name):
    text = (ROOT / path).read_text(encoding="utf-8")
    return int(re.search(rf"pub const {name}: u32 = ([0-9]+);", text).group(1))


@contextlib.contextmanager
def environment(**values):
    """Clear packaging overrides so the developer's shell cannot steer a test."""

    cleared = {
        local_release.IMAGE_ENV: "",
        local_release.REGISTRY_ENV: "",
        local_release.SIZE_ENV: "",
    }
    cleared.update(values)
    with patch.dict(os.environ, cleared, clear=False):
        yield


def result(returncode=0, stdout="", stderr=""):
    return subprocess.CompletedProcess([], returncode, stdout, stderr)


class DescriptorTests(unittest.TestCase):
    def test_descriptor_records_the_current_sources(self):
        with (
            environment(),
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
        self.assertEqual(payload["image_registry"], f"{DEFAULT_REGISTRY}:local-{version()}")
        self.assertEqual(payload["image_size"], 1024)
        self.assertRegex(payload["source_revision"], r"^[0-9a-f]{40}$")
        self.assertRegex(payload["built_at"], r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")
        self.assertNotIn("template_path", payload)

    def test_descriptor_records_the_version_pinned_ci_image_for_onboarding(self):
        with (
            environment(),
            patch.object(local_release, "version", return_value="2.3.4"),
            patch.object(local_release, "image_identity", return_value=(1024, "linux/amd64")),
            patch.object(local_release, "template", return_value=None),
        ):
            payload = local_release.descriptor()
            # The recorded reference must match the CI tag packaging itself would pull.
            expected = local_release.workbench_image()
        self.assertEqual(payload["image_registry"], "hlmark0/wukong-workbench:local-2.3.4")
        self.assertEqual(payload["image_registry"], expected)

    def test_descriptor_registry_can_be_overridden(self):
        override = "registry.example.com:5000/team/wukong-workbench"
        with (
            environment(**{local_release.REGISTRY_ENV: override}),
            patch.object(local_release, "version", return_value="2.3.4"),
            patch.object(local_release, "image_identity", return_value=(1024, "linux/amd64")),
            patch.object(local_release, "template", return_value=None),
        ):
            payload = local_release.descriptor()
        self.assertEqual(payload["image_registry"], override + ":local-2.3.4")

    def test_descriptor_refuses_the_official_release_repository(self):
        with (
            environment(**{local_release.REGISTRY_ENV: "hlmark0/wukong"}),
            patch.object(local_release, "image_identity", return_value=(1024, "linux/amd64")),
            patch.object(local_release, "template", return_value=None),
        ):
            with self.assertRaisesRegex(ValueError, "separate workbench repository"):
                local_release.descriptor()

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
        with (
            environment(),
            patch.object(local_release, "pull_workbench_image", return_value=False),
            patch.object(local_release, "_image_field", side_effect=[None, "sha256:abc"]) as field,
            patch.object(local_release.subprocess, "run", return_value=result()) as run,
        ):
            local_release.ensure_image("wukong:local")
        field.assert_any_call("wukong:local", "{{.Id}}")
        command = run.call_args.args[0]
        self.assertEqual(command[:4], ["docker", "buildx", "bake", "--load"])
        self.assertIn("workbench.tags=wukong:local", command)

    def test_a_failed_bake_reports_the_actionable_command(self):
        with (
            environment(),
            patch.object(local_release, "pull_workbench_image", return_value=False),
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
            environment(),
            patch.object(local_release, "pull_workbench_image", return_value=True),
            patch.object(local_release, "_image_field", side_effect=["windows/amd64"]),
        ):
            with self.assertRaisesRegex(ValueError, "linux/amd64"):
                local_release.image_identity("wukong:local")

    def test_image_size_must_be_positive(self):
        with (
            environment(),
            patch.object(local_release, "pull_workbench_image", return_value=True),
            patch.object(local_release, "_image_field", side_effect=["linux/amd64", "0"]),
        ):
            with self.assertRaisesRegex(ValueError, "reports no size"):
                local_release.image_identity("wukong:local")

    def test_unreachable_docker_records_unknown_size(self):
        daemon = "Cannot connect to the Docker daemon at unix:///var/run/docker.sock"
        with (
            environment(),
            patch.object(local_release, "_docker", return_value=result(1, stderr=daemon)),
        ):
            size, platform = local_release.image_identity("wukong:local")
        self.assertEqual(size, 0)
        self.assertEqual(platform, "unknown")

    def test_unreachable_docker_honours_the_size_override(self):
        daemon = "Cannot connect to the Docker daemon at unix:///var/run/docker.sock"
        with (
            environment(**{local_release.SIZE_ENV: "2048"}),
            patch.object(local_release, "_docker", return_value=result(1, stderr=daemon)),
        ):
            size, platform = local_release.image_identity("wukong:local")
        self.assertEqual(size, 2048)
        self.assertEqual(platform, "linux/amd64")

    def test_descriptor_can_record_an_unverified_size(self):
        with (
            environment(),
            patch.object(local_release, "image_identity", return_value=(0, "unknown")),
            patch.object(local_release, "template", return_value=None),
        ):
            payload = local_release.descriptor()
        self.assertEqual(payload["image_size"], 0)
        # Docker was unreachable at packaging time; onboarding can still pull this.
        self.assertEqual(payload["image_registry"], f"{DEFAULT_REGISTRY}:local-{version()}")

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
            "runtime_protocol": RUNTIME_PROTOCOL,
            "data_schema": 5,
            "image": "wukong:local",
            "image_size": 1024,
            "image_registry": "hlmark0/wukong-workbench:local-1.0.0",
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


class ImageSourceTests(unittest.TestCase):
    def test_ci_image_tag_uses_the_client_version_and_a_separate_repository(self):
        with environment(), patch.object(local_release, "version", return_value="2.3.4"):
            self.assertEqual(
                local_release.workbench_image(), "hlmark0/wukong-workbench:local-2.3.4"
            )
            self.assertEqual(local_release.registry(), DEFAULT_REGISTRY)
            # The official release image is <namespace>/wukong; this repository is not it.
            self.assertNotEqual(local_release.registry().rsplit("/", 1)[-1], "wukong")

    def test_ci_image_tag_uses_the_same_version_helper_as_the_descriptor(self):
        with environment():
            self.assertEqual(
                local_release.workbench_image(), f"{DEFAULT_REGISTRY}:local-{version()}"
            )

    def test_registry_can_be_overridden(self):
        override = "registry.example.com:5000/team/wukong-workbench"
        with (
            environment(**{local_release.REGISTRY_ENV: override}),
            patch.object(local_release, "version", return_value="2.3.4"),
        ):
            self.assertEqual(local_release.workbench_image(), override + ":local-2.3.4")

    def test_the_official_release_repository_is_refused(self):
        with environment(**{local_release.REGISTRY_ENV: "hlmark0/wukong"}):
            with self.assertRaisesRegex(ValueError, "separate workbench repository"):
                local_release.workbench_image()

    def test_an_explicit_override_uses_its_tag_without_pulling(self):
        with (
            environment(**{local_release.IMAGE_ENV: "wukong:dev"}),
            patch.object(local_release, "_image_field", return_value="sha256:id") as field,
            patch.object(local_release, "pull_workbench_image") as pull,
            patch.object(local_release.subprocess, "run") as run,
        ):
            local_release.ensure_image("wukong:dev")
        field.assert_called_once_with("wukong:dev", "{{.Id}}")
        pull.assert_not_called()
        run.assert_not_called()

    def test_an_explicit_override_wins_and_bakes_that_tag(self):
        with (
            environment(**{local_release.IMAGE_ENV: "wukong:dev"}),
            patch.object(local_release, "_image_field", side_effect=[None, "sha256:built"]),
            patch.object(local_release, "pull_workbench_image") as pull,
            patch.object(local_release.subprocess, "run") as run,
        ):
            local_release.ensure_image("wukong:dev")
        pull.assert_not_called()
        self.assertIn("workbench.tags=wukong:dev", run.call_args.args[0])

    def test_the_ci_image_is_pulled_verified_and_tagged_over_a_local_tag(self):
        events = []

        def docker(*arguments):
            events.append(arguments)
            return result()

        fields = iter(["linux/amd64", "sha256:tagged"])

        def image_field(image_name, template):
            events.append(("inspect", image_name, template))
            return next(fields)

        with (
            environment(),
            patch.object(local_release, "version", return_value="2.3.4"),
            patch.object(local_release, "_docker", side_effect=docker),
            patch.object(local_release, "_image_field", side_effect=image_field),
        ):
            local_release.ensure_image("wukong:local")

        reference = "hlmark0/wukong-workbench:local-2.3.4"
        self.assertEqual(
            events,
            [
                ("pull", "--platform", "linux/amd64", reference),
                ("inspect", reference, "{{.Os}}/{{.Architecture}}"),
                ("tag", reference, "wukong:local"),
                ("inspect", "wukong:local", "{{.Id}}"),
            ],
        )

    def test_image_identity_reads_the_pulled_image_platform_and_size(self):
        fields = iter(["linux/amd64", "sha256:tagged", "linux/amd64", "1048576"])
        with (
            environment(),
            patch.object(local_release, "version", return_value="2.3.4"),
            patch.object(local_release, "_docker", return_value=result()),
            patch.object(local_release, "_image_field", side_effect=lambda *args: next(fields)),
        ):
            size, platform = local_release.image_identity("wukong:local")
        self.assertEqual((size, platform), (1048576, "linux/amd64"))

    def test_a_published_image_without_an_amd64_manifest_is_refused(self):
        with (
            environment(),
            patch.object(local_release, "version", return_value="2.3.4"),
            patch.object(local_release, "_docker", return_value=result()) as docker,
            patch.object(local_release, "_image_field", side_effect=["linux/arm64"]),
        ):
            with self.assertRaisesRegex(ValueError, "linux/amd64"):
                local_release.ensure_image("wukong:local")
        self.assertFalse(any("tag" in call.args for call in docker.call_args_list))

    def test_missing_auth_and_network_pull_failures_reuse_a_local_tag(self):
        cases = [
            ("Error response from daemon: manifest unknown", "not found"),
            (
                "Error response from daemon: unauthorized: authentication required",
                "not authorized",
            ),
            ('Get "https://registry-1.docker.io/v2/": dial tcp: no such host', "network"),
        ]
        for stderr, reason in cases:
            with self.subTest(reason=reason), environment():
                events = []

                def docker(*arguments):
                    events.append(arguments)
                    return result(returncode=1, stderr=stderr)

                errors = io.StringIO()
                with (
                    patch.object(local_release, "_docker", side_effect=docker),
                    patch.object(local_release, "_image_field", return_value="sha256:existing"),
                    patch.object(local_release.subprocess, "run") as bake,
                    contextlib.redirect_stderr(errors),
                ):
                    local_release.ensure_image("wukong:local")
                bake.assert_not_called()
                self.assertEqual(events[0][:1], ("pull",))
                warning = errors.getvalue()
                self.assertIn(reason, warning)
                self.assertIn("existing local Docker image wukong:local", warning)
                self.assertIn("older build", warning)
                self.assertIn("docker image rm wukong:local", warning)

    def test_missing_auth_and_network_pull_failures_warn_then_bake(self):
        cases = [
            ("Error response from daemon: manifest unknown", "not found"),
            (
                "Error response from daemon: unauthorized: authentication required",
                "not authorized",
            ),
            ('Get "https://registry-1.docker.io/v2/": dial tcp: no such host', "network"),
        ]
        for stderr, reason in cases:
            with self.subTest(reason=reason), environment():
                events = []

                def docker(*arguments):
                    events.append(arguments)
                    return result(returncode=1, stderr=stderr)

                errors = io.StringIO()
                with (
                    patch.object(local_release, "_docker", side_effect=docker),
                    patch.object(local_release, "_image_field", side_effect=[None, "sha256:built"]),
                    patch.object(local_release.subprocess, "run") as run,
                    contextlib.redirect_stderr(errors),
                ):
                    local_release.ensure_image("wukong:local")
                self.assertEqual(events[0][:1], ("pull",))
                self.assertIn("workbench.tags=wukong:local", run.call_args.args[0])
                warning = errors.getvalue()
                self.assertIn(reason, warning)
                self.assertIn("maintainer", warning)
                self.assertIn("Workbench image", warning)
                self.assertIn("docker buildx bake", warning)

    def test_an_unexpected_pull_failure_is_reported_instead_of_hidden(self):
        with (
            environment(),
            patch.object(
                local_release,
                "_docker",
                return_value=result(returncode=1, stderr="invalid reference format"),
            ),
        ):
            with self.assertRaisesRegex(ValueError, "Cannot pull"):
                local_release.ensure_image("wukong:local")

    def test_an_unreachable_daemon_defers_instead_of_baking(self):
        errors = io.StringIO()
        daemon = "Cannot connect to the Docker daemon at unix:///var/run/docker.sock"
        with (
            environment(),
            patch.object(local_release, "_docker", return_value=result(1, stderr=daemon)),
            patch.object(local_release.subprocess, "run") as run,
            contextlib.redirect_stderr(errors),
        ):
            local_release.ensure_image("wukong:local")
        run.assert_not_called()
        self.assertIn("Docker is not reachable", errors.getvalue())


if __name__ == "__main__":
    unittest.main()
