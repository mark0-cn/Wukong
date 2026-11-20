import json
import os
import unittest
from unittest.mock import patch

from scripts._lib.versioning import ROOT
from scripts.release import build_installer


class ConfigurationTests(unittest.TestCase):
    def test_windows_bundle_includes_worker_setup_resource(self):
        self.assertEqual(
            json.loads(build_installer.configuration("windows")),
            {
                "bundle": {
                    "createUpdaterArtifacts": False,
                    "resources": {
                        "target/worker-setup/WukongWorkerSetup.exe": (
                            "worker-setup/WukongWorkerSetup.exe"
                        )
                    },
                }
            },
        )

    def test_windows_signed_configuration_matches_the_ci_overlay(self):
        self.assertEqual(
            build_installer.configuration("windows", signed=True),
            '{"bundle":{"createUpdaterArtifacts":true,"resources":'
            '{"target/worker-setup/WukongWorkerSetup.exe":'
            '"worker-setup/WukongWorkerSetup.exe"}}}',
        )

    def test_linux_bundle_keeps_the_updater_only_configuration(self):
        self.assertEqual(
            json.loads(build_installer.configuration("linux", signed=True)),
            {"bundle": {"createUpdaterArtifacts": True}},
        )

    def test_local_release_overlay_adds_the_descriptor_resource(self):
        windows = json.loads(build_installer.configuration("windows", local_release=True))
        self.assertEqual(
            windows["bundle"]["resources"],
            {
                "target/worker-setup/WukongWorkerSetup.exe": "worker-setup/WukongWorkerSetup.exe",
                "target/local-release.json": "local-release.json",
            },
        )
        linux = json.loads(build_installer.configuration("linux", local_release=True))
        self.assertEqual(
            linux["bundle"]["resources"],
            {"target/local-release.json": "local-release.json"},
        )

    def test_ci_default_configuration_has_no_local_release_payload(self):
        """The release packaging path must never enable the local-release mode."""

        for platform in ("windows", "linux"):
            configuration = json.loads(build_installer.configuration(platform))
            self.assertNotIn("local-release.json", json.dumps(configuration))
            self.assertNotIn("local-release", json.dumps(configuration["bundle"]))


class BuildTests(unittest.TestCase):
    def test_windows_build_stages_the_setup_before_bundling(self):
        events = []

        environment = {"PATH": "C:/msvc", "TMP": "D:/work/target/tmp"}

        def stage(output, environment):
            events.append(("stage", output, environment))

        def run(command, **kwargs):
            events.append(("run", command, kwargs))

        with (
            patch.object(build_installer.worker_setup, "build_and_stage", side_effect=stage),
            patch.object(
                build_installer.worker_setup, "build_environment", return_value=environment
            ),
            patch.object(build_installer.msvc, "preflight") as preflight,
            patch.object(build_installer.subprocess, "run", side_effect=run),
            patch.object(build_installer, "prepare_local_release") as prepare,
        ):
            build_installer.build("windows")
        prepare.assert_not_called()
        preflight.assert_called_once_with(environment, ROOT)
        self.assertEqual(events[0], ("stage", ROOT / "target/worker-setup", environment))
        self.assertEqual(events[1][1][1:], ["ci"])
        self.assertEqual(events[1][2]["env"], environment)
        tauri = events[2][1]
        self.assertEqual(events[2][2]["env"], environment)
        configuration = json.loads(tauri[tauri.index("--config") + 1])
        self.assertEqual(
            configuration["bundle"]["resources"],
            {"target/worker-setup/WukongWorkerSetup.exe": "worker-setup/WukongWorkerSetup.exe"},
        )
        self.assertEqual(tauri[-2:], ["--", "--locked"])
        self.assertEqual(tauri[1:4], ["--no-install", "tauri", "build"])
        self.assertNotIn("local-release", tauri)

    def test_local_release_build_prepares_the_descriptor_and_enables_the_feature(self):
        events = []

        def run(command, **kwargs):
            events.append(command)

        with (
            patch.object(build_installer.worker_setup, "build_and_stage"),
            patch.object(
                build_installer.worker_setup, "build_environment", return_value={"PATH": "C:/msvc"}
            ),
            patch.object(build_installer.msvc, "preflight"),
            patch.object(build_installer.subprocess, "run", side_effect=run),
            patch.object(
                build_installer, "prepare_local_release", return_value=(ROOT / "x", {})
            ) as prepare,
        ):
            build_installer.build("windows", local_release=True)
        prepare.assert_called_once_with()
        tauri = next(command for command in events if "tauri" in command)
        self.assertEqual(tauri[tauri.index("--features") + 1], "custom-protocol,local-release")
        configuration = json.loads(tauri[tauri.index("--config") + 1])
        self.assertEqual(
            configuration["bundle"]["resources"]["target/local-release.json"],
            "local-release.json",
        )

    def test_linux_build_does_not_stage_the_setup(self):
        with (
            patch.object(build_installer.worker_setup, "build_and_stage") as stage,
            patch.object(build_installer.subprocess, "run") as run,
            patch.dict(os.environ, {"TAURI_SIGNING_PRIVATE_KEY": "key"}, clear=False),
        ):
            build_installer.build("linux", signed=True)
        stage.assert_not_called()
        configuration = json.loads(run.call_args_list[1].args[0][-3])
        self.assertNotIn("resources", configuration["bundle"])
        self.assertTrue(configuration["bundle"]["createUpdaterArtifacts"])

    def test_signed_windows_build_requires_a_signing_key(self):
        with (
            patch.object(build_installer.worker_setup, "build_and_stage") as stage,
            patch.dict(os.environ, {"TAURI_SIGNING_PRIVATE_KEY": ""}, clear=False),
        ):
            with self.assertRaisesRegex(ValueError, "TAURI_SIGNING_PRIVATE_KEY"):
                build_installer.build("windows", signed=True)
        stage.assert_not_called()


class ReleaseWorkflowGuardTests(unittest.TestCase):
    def test_ci_workflow_never_enables_local_release(self):
        for name in ("build-installer.yml", "ci.yml", "release.yml"):
            workflow = (ROOT / ".github/workflows" / name).read_text(encoding="utf-8")
            self.assertNotIn("local-release", workflow, name)
            self.assertNotIn("package-local", workflow, name)


if __name__ == "__main__":
    unittest.main()
