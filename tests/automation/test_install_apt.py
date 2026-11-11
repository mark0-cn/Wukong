"""Regress the hosted-runner DNS failure without changing the developer's APT."""
import os
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch
from scripts.ci import install_apt as apt


class InstallAptTests(unittest.TestCase):
    def test_non_hosted_or_unsupported_runner_never_runs_a_command(self):
        hosted = dict(GITHUB_ACTIONS="true", RUNNER_ENVIRONMENT="github-hosted", RUNNER_OS="Linux")
        cases = [({}, "Linux", "ubuntu", "noble", "x86_64"),
                 ({**hosted, "RUNNER_ENVIRONMENT": "self-hosted"}, "Linux", "ubuntu", "noble", "x86_64"),
                 (hosted, "Windows", "ubuntu", "noble", "x86_64"),
                 (hosted, "Linux", "debian", "noble", "x86_64"),
                 (hosted, "Linux", "ubuntu", "future", "x86_64"),
                 (hosted, "Linux", "ubuntu", "noble", "aarch64")]
        for env, system, distro, suite, machine in cases:
            with self.subTest(env=env, suite=suite), patch.dict(os.environ, env, clear=True), \
                    patch.object(apt.platform, "system", return_value=system), \
                    patch.object(apt.platform, "freedesktop_os_release", create=True, return_value=dict(ID=distro, VERSION_CODENAME=suite)), \
                    patch.object(apt.platform, "machine", return_value=machine), \
                    patch.object(apt.subprocess, "run") as run:
                with self.assertRaises(ValueError):
                    apt.install(["gdb"])
                run.assert_not_called()

    def test_official_sources_keep_signatures_and_reject_untrusted_suite(self):
        for suite in ("jammy", "noble"):
            source = apt.sources(suite)
            self.assertIn("https://archive.ubuntu.com/ubuntu", source)
            self.assertIn("https://security.ubuntu.com/ubuntu", source)
            self.assertEqual(source.count("Signed-By: " + apt.KEYRING), 2)
            self.assertIn(f"Suites: {suite}-security", source)
            self.assertNotIn("trusted", source)
        with self.assertRaises(ValueError):
            apt.sources("noble\nTrusted: yes")
        config = apt.configuration()
        self.assertIn('APT::Update::Error-Mode "any";', config)
        self.assertIn('Acquire::https::Timeout "15";', config)
        self.assertIn('Acquire::Retries "2";', config)
        self.assertNotIn("AllowUnauthenticated", config)

    def test_bad_index_update_or_timeout_prevents_install_and_fails(self):
        for code, expected in ((100, "exited 100"), (124, "timed out"), (137, "timed out")):
            commands = []

            def run(command, **kwargs):
                commands.append(command)
                return subprocess.CompletedProcess(command, code if "apt-get" in command else 0)

            with self.subTest(code=code), patch.object(apt, "require_runner", return_value="noble"), \
                    patch.object(apt.subprocess, "run", side_effect=run):
                with self.assertRaisesRegex(RuntimeError, expected):
                    apt.install(["gdb"])
            apt_commands = [command for command in commands if "apt-get" in command]
            self.assertEqual(len(apt_commands), 1)
            self.assertIn("120s", apt_commands[0])
            self.assertIn("--kill-after=15s", apt_commands[0])
            self.assertEqual(apt_commands[0][-1], "update")

    def test_success_installs_only_explicit_packages_after_strict_update(self):
        commands, config_files = [], {}

        def run(command, **kwargs):
            commands.append(command)
            if "644" in command:
                config_files[command[-1]] = Path(command[-2]).read_text(encoding="utf-8")
            return subprocess.CompletedProcess(command, 0)

        with patch.object(apt, "require_runner", return_value="jammy"), \
                patch.object(apt.subprocess, "run", side_effect=run):
            apt.install(["gdb", "libwebkit2gtk-4.1-dev"])
        self.assertEqual(config_files[apt.SOURCES], apt.sources("jammy"))
        self.assertEqual(config_files[apt.CONFIG], apt.configuration())
        install = commands[-1]
        self.assertIn("480s", install)
        self.assertEqual(install[-4:], ["install", "-y", "gdb", "libwebkit2gtk-4.1-dev"])
        for package in ("--allow-unauthenticated", "gdb-", "../gdb.deb", "gdb;echo", "gdb=1"):
            with patch.object(apt.subprocess, "run") as run:
                with self.assertRaises(ValueError):
                    apt.install([package])
                run.assert_not_called()
