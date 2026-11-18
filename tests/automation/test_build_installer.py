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


class BuildTests(unittest.TestCase):
    def test_windows_build_stages_the_setup_before_bundling(self):
        events = []

        def stage(output):
            events.append(("stage", output))

        def run(command, **kwargs):
            events.append(("run", command))

        with (
            patch.object(build_installer.worker_setup, "build_and_stage", side_effect=stage),
            patch.object(build_installer.subprocess, "run", side_effect=run),
        ):
            build_installer.build("windows")
        self.assertEqual(events[0], ("stage", ROOT / "target/worker-setup"))
        self.assertEqual(events[1][1][1:], ["ci"])
        tauri = events[2][1]
        configuration = json.loads(tauri[tauri.index("--config") + 1])
        self.assertEqual(
            configuration["bundle"]["resources"],
            {"target/worker-setup/WukongWorkerSetup.exe": "worker-setup/WukongWorkerSetup.exe"},
        )
        self.assertEqual(tauri[-2:], ["--", "--locked"])
        self.assertEqual(tauri[1:4], ["--no-install", "tauri", "build"])

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


if __name__ == "__main__":
    unittest.main()
