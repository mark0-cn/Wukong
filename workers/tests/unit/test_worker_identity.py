"""Actual file-backed identity and fencing; no target execution or VM required."""
from pathlib import Path
import os
import tempfile
import unittest
from types import SimpleNamespace
import threading

from worker_common import WorkerError
from worker_identity import ExclusiveLease, installation_identity, native_package_identity, boot_identity


class WorkerFenceTests(unittest.TestCase):
    @unittest.skipUnless(os.name=='nt','Windows boot identity requires Windows')
    def test_real_windows_boot_identity_is_available_and_stable(self):
        first=boot_identity()
        self.assertRegex(first,r'^windows:[0-9]{16,20}$')
        self.assertEqual(first,boot_identity())

    def setUp(self):
        self.temporary=tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root=Path(self.temporary.name)
        self.now=1000
        self.active=False
        self.identity=SimpleNamespace(value={'boot_id':'boot-one'})
        self.identity.validate=lambda binding: self.assertEqual(binding,self.identity.value)
        self.lease=ExclusiveLease(self.root,self.identity,lambda:self.active,lambda:self.now)

    def acquire(self,run='run',owner='controller'):
        return self.lease.acquire({'run_id':run,'owner_id':owner})

    def test_installation_identity_is_persistent_and_never_repaired_silently(self):
        self.assertEqual(installation_identity(self.root),installation_identity(self.root))
        (self.root/'installation.json').write_text('{"installation_id":"invalid"}')
        with self.assertRaises(ValueError): installation_identity(self.root)

    def test_native_binding_uses_whole_signed_package_and_rejects_cross_installation(self):
        installation='a'*32
        binding={'schema':1,'protocol':2,'installation_id':installation,'package_sha256':'b'*64}
        self.assertEqual(native_package_identity(binding,installation),'b'*64)
        for changed in [dict(binding,installation_id='c'*32),dict(binding,protocol=1),
                        dict(binding,package_sha256='B'*64),dict(binding,unexpected=True)]:
            with self.assertRaises(ValueError): native_package_identity(changed,installation)

    def test_unisolated_worker_does_not_advertise_native_execution(self):
        from worker_capabilities import doctor
        capabilities=doctor(self.root,False,False)['capabilities']
        for name in ['process_jobs','persistent_jobs','crash_replay','poc_verify','shell_exec','py_run','native_target_execution']:
            self.assertFalse(capabilities[name],name)

    def test_windows_probe_keeps_portable_analysis_but_excludes_linux_engines(self):
        from unittest.mock import patch
        from worker_capabilities import doctor
        with patch('worker_capabilities.sys', SimpleNamespace(platform='win32')), \
                patch('worker_capabilities.platform.system', return_value='Windows'), \
                patch('worker_dbgeng.doctor', return_value={'available':True}), \
                patch('worker_capabilities._python_module', return_value=True):
            capabilities=doctor(self.root,True,False)['capabilities']
        self.assertTrue(capabilities['debug_dbgeng'])
        self.assertTrue(capabilities['resumable_file_transfer'])
        for name in ['debug_gdb_mi','fuzz_qemu','ghidra_query','core_snapshot']:
            self.assertFalse(capabilities[name],name)
        for name in ['seccomp_analyze','runtime_profile','libc_identify','crash_replay','poc_verify']:
            self.assertTrue(capabilities[name],name)

    def test_live_lease_rejects_other_controllers_and_renews_the_same_fence(self):
        first=self.acquire()
        with self.assertRaises(WorkerError): self.acquire(owner='other')
        self.now+=20
        renewed=self.acquire()
        self.assertEqual(renewed['epoch'],first['epoch'])
        self.assertEqual(renewed['token'],first['token'])
        self.lease.validate(first)
        self.assertGreater(renewed['expires_at'],first['expires_at'])

    def test_expired_owner_cannot_execute_or_release_successor_even_with_same_name(self):
        first=self.acquire()
        self.now+=121
        next_lease=self.acquire()
        self.assertGreater(next_lease['epoch'],first['epoch'])
        with self.assertRaises(WorkerError): self.lease.validate(first)
        with self.assertRaises(WorkerError): self.lease.release({'lease':first})
        self.lease.validate(next_lease)

    def test_expiry_does_not_prove_processes_died(self):
        first=self.acquire()
        self.active=True
        with self.assertRaises(WorkerError): self.lease.release({'lease':first})
        self.now+=121
        with self.assertRaises(WorkerError): self.acquire(run='another')
        recovered=self.acquire(owner='successor')
        self.lease.validate(recovered)
        self.active=False
        self.lease.release({'lease':recovered})
        self.assertGreater(self.acquire(run='another')['epoch'],recovered['epoch'])

    def test_restart_and_boot_change_preserve_monotonic_epochs(self):
        first=self.acquire()
        self.lease=ExclusiveLease(self.root,self.identity,lambda:False,lambda:self.now)
        with self.assertRaises(WorkerError): self.acquire(owner='other')
        self.identity.value={'boot_id':'boot-two'}
        second=self.acquire(owner='other')
        self.assertGreater(second['epoch'],first['epoch'])
        with self.assertRaises(WorkerError): self.lease.validate(first)

    def test_fence_is_checked_before_any_cached_effect_can_be_returned(self):
        first=self.acquire()
        self.now+=121
        self.acquire(owner='successor')
        calls=[]
        with self.assertRaises(WorkerError):
            self.lease.dispatch({'_binding':self.identity.value,'_lease':first},lambda params:calls.append(params))
        self.assertEqual(calls,[])

    def test_volatile_fence_is_removed_from_the_business_receipt_fingerprint(self):
        token=self.acquire()
        result=self.lease.dispatch({'_binding':self.identity.value,'_lease':token,'request_id':'stable','target':'file'},lambda params:params)
        self.assertEqual(result,{'request_id':'stable','target':'file'})

    def test_slow_operation_allows_renewal_but_blocks_expired_takeover(self):
        token=self.acquire()
        entered=threading.Event();finish=threading.Event()
        def operation(params):
            entered.set()
            if not finish.wait(5): raise AssertionError('test operation was not released')
        thread=threading.Thread(target=lambda:self.lease.dispatch({'_binding':self.identity.value,'_lease':token},operation))
        thread.start()
        try:
            self.assertTrue(entered.wait(2))
            self.now+=20
            self.assertEqual(self.acquire()['epoch'],token['epoch'])
            self.now+=121
            with self.assertRaises(WorkerError): self.acquire(owner='successor')
        finally:
            finish.set();thread.join(2)
        self.assertFalse(thread.is_alive())
        self.assertGreater(self.acquire(owner='successor')['epoch'],token['epoch'])
