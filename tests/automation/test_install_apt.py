"""Dependency admission, whole-mirror retries and offline installation invariants."""

import os
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch
from scripts.ci import install_apt as apt


class InstallAptTests(unittest.TestCase):
    def test_non_hosted_or_unsupported_runner_never_runs_a_command(self):
        hosted = dict(GITHUB_ACTIONS="true", RUNNER_ENVIRONMENT="github-hosted", RUNNER_OS="Linux")
        cases = [
            ({}, "Linux", "ubuntu", "noble", "x86_64"),
            ({**hosted, "RUNNER_ENVIRONMENT": "self-hosted"}, "Linux", "ubuntu", "noble", "x86_64"),
            (hosted, "Windows", "ubuntu", "noble", "x86_64"),
            (hosted, "Linux", "debian", "noble", "x86_64"),
            (hosted, "Linux", "ubuntu", "future", "x86_64"),
            (hosted, "Linux", "ubuntu", "noble", "aarch64"),
        ]
        for env, system, distro, suite, machine in cases:
            with (
                self.subTest(env=env, suite=suite),
                patch.dict(os.environ, env, clear=True),
                patch.object(apt.platform, "system", return_value=system),
                patch.object(
                    apt.platform,
                    "freedesktop_os_release",
                    create=True,
                    return_value=dict(ID=distro, VERSION_CODENAME=suite),
                ),
                patch.object(apt.platform, "machine", return_value=machine),
                patch.object(apt.subprocess, "run") as run,
            ):
                with self.assertRaises(ValueError):
                    apt.install(["gdb"])
                run.assert_not_called()

    def test_signed_direct_sources_and_minimal_indexes(self):
        for suite in ("jammy", "noble"):
            for mirror in apt.MIRRORS:
                source = apt.sources(suite, mirror)
                self.assertNotIn("mirror+file", source)
                self.assertEqual(source.count("Signed-By: " + apt.KEYRING), 2)
                self.assertIn(f"{suite}-security", source)
                self.assertNotIn("trusted", source)
            canonical = apt.sources(suite, apt.MIRRORS[-1])
            self.assertIn("https://security.ubuntu.com/ubuntu/", canonical)
        for suite, mirror in [
            ("future", apt.MIRRORS[0]),
            ("noble", "http://untrusted/ubuntu/"),
            ("noble", apt.MIRRORS[0] + "\nTrusted: yes"),
        ]:
            with self.assertRaises(ValueError):
                apt.sources(suite, mirror)
        config = apt.configuration("/isolated/lists")
        self.assertIn('APT::Update::Error-Mode "any";', config)
        self.assertIn('Acquire::Retries "0";', config)
        self.assertIn('Acquire::Languages "none";', config)
        self.assertIn('Dir::State::lists "/isolated/lists";', config)
        self.assertNotIn("AllowUnauthenticated", config)

    def test_preinstalled_packages_skip_network_but_are_probed(self):
        with (
            patch.object(apt, "require_runner", return_value="noble"),
            patch.object(apt, "installed", return_value=True),
            patch.object(apt, "download") as download,
            patch.object(apt, "configure") as configure,
            patch.object(apt.subprocess, "run") as run,
        ):
            apt.install(["gdb", "gdb", "libwayland-dev"])
        download.assert_not_called()
        configure.assert_not_called()
        self.assertEqual(
            [call.args[0] for call in run.call_args_list],
            [["gdb", "--version"], ["pkg-config", "--modversion", "wayland-client"]],
        )

    def test_only_missing_packages_download_and_install_without_network(self):
        with (
            patch.object(apt, "require_runner", return_value="noble"),
            patch.object(apt, "installed", side_effect=lambda p: p == "gdb"),
            patch.object(apt, "download") as download,
            patch.object(apt, "apt") as command,
            patch.object(apt, "verify") as verify,
        ):
            apt.install(["gdb", "musl-tools", "musl-tools"])
        self.assertEqual(download.call_args.args[0], ["musl-tools"])
        command.assert_called_once_with(
            ["install", "--no-download", "-y", "musl-tools"], apt.INSTALL_SECONDS
        )
        verify.assert_called_once_with(["gdb", "musl-tools"])

    def test_failed_index_or_package_download_abandons_entire_mirror(self):
        for failing in ["update", "download"]:
            configured, calls = [], []

            def command(arguments, seconds):
                calls.append((configured[-1], arguments, seconds))
                if configured[-1] == "bad" and (
                    arguments[0] == "update"
                    if failing == "update"
                    else "--download-only" in arguments
                ):
                    raise RuntimeError("fixture read timed out")

            with (
                self.subTest(phase=failing),
                patch.object(
                    apt, "configure", side_effect=lambda source, lists: configured.append(source)
                ) as configure,
                patch.object(apt, "apt", side_effect=command),
            ):
                apt.download(["gdb"], [("first", "bad"), ("second", "good")])
            self.assertEqual(configured, ["bad", "good"])
            paths = [call.args[1] for call in configure.call_args_list]
            self.assertNotEqual(
                paths[0], paths[1], "partial indexes must not carry across attempts"
            )
            self.assertEqual(calls[-1][0], "good")
            self.assertIn("--download-only", calls[-1][1])
            self.assertFalse(any("--no-download" in call[1] for call in calls))

    def test_network_budget_and_all_mirror_failure_prevent_install(self):
        with (
            patch.object(apt, "require_runner", return_value="noble"),
            patch.object(apt, "installed", return_value=False),
            patch.object(apt, "configure"),
            patch.object(apt, "apt", side_effect=RuntimeError("fixture failure")) as command,
        ):
            with self.assertRaisesRegex(RuntimeError, "All APT mirror attempts failed"):
                apt.install(["gdb"])
            self.assertEqual(command.call_count, len(apt.MIRRORS))
            self.assertTrue(all(call.args[0] == ["update"] for call in command.call_args_list))
        with (
            patch.object(apt.time, "monotonic", side_effect=[0, 1, apt.NETWORK_SECONDS]),
            patch.object(apt, "configure"),
            patch.object(apt, "apt") as command,
        ):
            with self.assertRaisesRegex(RuntimeError, "network budget exhausted"):
                apt.download(["gdb"], [("only", "source")])
            command.assert_not_called()

    def test_success_exit_does_not_hide_missing_packages_or_broken_tools(self):
        with patch.object(apt, "installed", return_value=False):
            with self.assertRaisesRegex(RuntimeError, "not installed"):
                apt.verify(["gdb"])
        with (
            patch.object(apt, "installed", return_value=True),
            patch.object(
                apt.subprocess, "run", side_effect=subprocess.CalledProcessError(1, "gdb")
            ),
        ):
            with self.assertRaises(subprocess.CalledProcessError):
                apt.verify(["gdb"])

    def test_attempt_configuration_changes_only_job_owned_paths(self):
        commands, files = [], {}

        def run(command, **kwargs):
            commands.append(command)
            if "644" in command:
                files[command[-1]] = Path(command[-2]).read_text(encoding="utf-8")
            return subprocess.CompletedProcess(command, 0)

        with patch.object(apt.subprocess, "run", side_effect=run):
            apt.configure("signed source", "/isolated/lists")
        self.assertEqual(files[apt.SOURCES], "signed source")
        self.assertEqual(files[apt.CONFIG], apt.configuration("/isolated/lists"))
        self.assertEqual(len(commands), 3)

    def test_command_deadlines_and_invalid_packages(self):
        for code, expected in [(100, "exited 100"), (124, "timed out"), (137, "timed out")]:
            with patch.object(
                apt.subprocess, "run", return_value=subprocess.CompletedProcess([], code)
            ) as run:
                with self.assertRaisesRegex(RuntimeError, expected):
                    apt.apt(["update"], 4)
                self.assertIn("4s", run.call_args.args[0])
                self.assertIn("--kill-after=5s", run.call_args.args[0])
        for package in ["--allow-unauthenticated", "gdb-", "../gdb.deb", "gdb;echo", "gdb=1"]:
            with patch.object(apt.subprocess, "run") as run:
                with self.assertRaises(ValueError):
                    apt.install([package])
                run.assert_not_called()
