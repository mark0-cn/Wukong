"""The global installation fixture must reject a developer host before effects."""
import importlib.util
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('hosted_worker_fixture', Path(__file__).parents[1] / 'acceptance/check_windows_worker.py')
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)


class HostedWorkerGuardTests(unittest.TestCase):
    def test_public_diagnostics_exclude_private_records_and_bound_trusted_logs(self):
        import json
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            metadata = root / 'metadata'; metadata.mkdir()
            for name in ['endpoint.json','exclusive-lease.json','authority.json','ssh_host_ed25519_key']:
                (metadata/name).write_text('CONTROLLED-PRIVATE-CANARY-NOT-A-CREDENTIAL', encoding='utf-8')
            (metadata/'last-service-error.txt').write_text('known fixture failure',encoding='utf-8')
            (metadata/'selftest-fixture.log').write_text('x'*9000,encoding='utf-8')
            report = fixture.diagnostics(root)
            self.assertEqual(report['last-service-error.txt'],'known fixture failure')
            self.assertEqual(len(report['selftest-fixture.log']),4096)
            self.assertNotIn('CONTROLLED-PRIVATE-CANARY',json.dumps(report))

    def test_default_and_self_hosted_environments_cannot_reach_installation_effects(self):
        for environment in [{}, {'GITHUB_ACTIONS':'true','RUNNER_ENVIRONMENT':'self-hosted'}]:
            with self.subTest(environment=environment), patch.dict(os.environ, environment, clear=True), \
                    patch.object(fixture.platform, 'system', return_value='Windows'), \
                    patch.object(fixture.tempfile, 'mkdtemp', side_effect=AssertionError('host mutation')), \
                    patch.object(sys, 'argv', ['fixture', '--confirm-hosted-runner', '--report', 'never-created.json']):
                with self.assertRaisesRegex(RuntimeError, 'dedicated GitHub-hosted'):
                    fixture.main()

    def test_explicit_confirmation_and_windows_are_required_even_with_hosted_environment(self):
        with tempfile.TemporaryDirectory() as temporary:
            environment = {'GITHUB_ACTIONS':'true','RUNNER_ENVIRONMENT':'github-hosted','RUNNER_TEMP':temporary}
            with patch.dict(os.environ, environment, clear=True), patch.object(fixture.platform, 'system', return_value='Windows'):
                with self.assertRaises(RuntimeError): fixture.require_hosted(False)
                self.assertEqual(fixture.require_hosted(True), Path(temporary).resolve())
            with patch.dict(os.environ, environment, clear=True), patch.object(fixture.platform, 'system', return_value='Linux'):
                with self.assertRaises(RuntimeError): fixture.require_hosted(True)
