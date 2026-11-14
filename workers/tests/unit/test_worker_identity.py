"""Actual file-backed fixture/Linux identity; no target execution or VM required."""
from pathlib import Path
import os
import tempfile
import unittest
from types import SimpleNamespace

from worker_identity import installation_identity, boot_identity


class WorkerFenceTests(unittest.TestCase):
    @unittest.skipUnless(os.name=='nt','Windows boot identity requires Windows')
    def test_real_windows_boot_identity_is_available_and_stable(self):
        from unittest.mock import patch
        boot_identity.cache_clear()
        with patch('subprocess.run',side_effect=AssertionError('boot identity must not spawn a shell')):
            first=boot_identity()
            boot_identity.cache_clear()
            self.assertEqual(first,boot_identity())
        self.assertRegex(first,r'^windows:[0-9a-f]{32}:[0-9]{1,10}$')

    @unittest.skipUnless(os.name=='nt','Windows boot identity requires Windows')
    def test_native_windows_boot_epoch_matches_a_fresh_minimal_environment_process(self):
        import subprocess,sys
        from worker_common import safe_env
        runtime=Path(__file__).resolve().parents[2]/'runtime'
        code=f"import sys;sys.path.insert(0,{str(runtime)!r});from worker_identity import boot_identity;print(boot_identity())"
        result=subprocess.run([sys.executable,'-I','-c',code],env=safe_env(),capture_output=True,text=True,timeout=10)
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(result.stdout.strip(),boot_identity())

    def setUp(self):
        self.temporary=tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root=Path(self.temporary.name)
    def test_installation_identity_is_persistent_and_never_repaired_silently(self):
        self.assertEqual(installation_identity(self.root),installation_identity(self.root))
        (self.root/'installation.json').write_text('{"installation_id":"invalid"}')
        with self.assertRaises(ValueError): installation_identity(self.root)

    def test_unisolated_worker_does_not_advertise_native_execution(self):
        from worker_capabilities import doctor
        from worker_runtime import Runtime
        capabilities=doctor(self.root,False,False,Runtime(self.root,True).registrations)['capabilities']
        for name in ['process_jobs','persistent_jobs','crash_replay','poc_verify','shell_exec','py_run','native_target_execution']:
            self.assertFalse(capabilities[name],name)

    def test_windows_probe_keeps_portable_analysis_but_excludes_linux_engines(self):
        from unittest.mock import patch
        from worker_capabilities import doctor
        from worker_runtime import Runtime
        with patch('worker_capabilities.sys', SimpleNamespace(platform='win32')), \
                patch('worker_capabilities.platform.system', return_value='Windows'), \
                patch('worker_capabilities._python_module', return_value=True):
            capabilities=doctor(self.root,True,False,Runtime(self.root,True).registrations)['capabilities']
        self.assertFalse(capabilities['debug_dbgeng'])
        self.assertTrue(capabilities['resumable_file_transfer'])
        for name in ['debug_gdb_mi','fuzz_qemu','ghidra_query','core_snapshot']:
            self.assertFalse(capabilities[name],name)
        for name in ['seccomp_analyze','runtime_profile','libc_identify','crash_replay','poc_verify']:
            self.assertTrue(capabilities[name],name)
