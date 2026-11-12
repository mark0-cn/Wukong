"""Real ordinary-token path checks without accounts or machine policy changes."""
from contextlib import contextmanager
import ctypes
from ctypes import wintypes
import os
from pathlib import Path
import tempfile
import unittest

import worker_dbgeng


@contextmanager
def without_privileges():
    class Luid(ctypes.Structure):
        _fields_ = [("low", wintypes.DWORD), ("high", wintypes.LONG)]
    class Privilege(ctypes.Structure):
        _fields_ = [("luid", Luid), ("attributes", wintypes.DWORD)]
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    security = ctypes.WinDLL("advapi32", use_last_error=True)
    kernel.GetCurrentProcess.restype = wintypes.HANDLE
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    security.OpenProcessToken.argtypes = [wintypes.HANDLE, wintypes.DWORD, ctypes.POINTER(wintypes.HANDLE)]
    security.GetTokenInformation.argtypes = [wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p, wintypes.DWORD, ctypes.POINTER(wintypes.DWORD)]
    security.CreateRestrictedToken.argtypes = [wintypes.HANDLE, wintypes.DWORD, wintypes.DWORD, ctypes.c_void_p,
        wintypes.DWORD, ctypes.POINTER(Privilege), wintypes.DWORD, ctypes.c_void_p, ctypes.POINTER(wintypes.HANDLE)]
    security.ImpersonateLoggedOnUser.argtypes = [wintypes.HANDLE]
    security.RevertToSelf.argtypes = []
    def checked(result):
        if not result:
            raise ctypes.WinError(ctypes.get_last_error())
    def privileges(token):
        size = wintypes.DWORD()
        security.GetTokenInformation(token, 3, None, 0, ctypes.byref(size))
        buffer = ctypes.create_string_buffer(size.value)
        checked(security.GetTokenInformation(token, 3, buffer, size, ctypes.byref(size)))
        return buffer
    original, restricted = wintypes.HANDLE(), wintypes.HANDLE()
    active = False
    try:
        checked(security.OpenProcessToken(kernel.GetCurrentProcess(), 0xA, ctypes.byref(original)))
        buffer = privileges(original)
        count = wintypes.DWORD.from_buffer(buffer).value
        deletion = ctypes.cast(ctypes.addressof(buffer) + 4, ctypes.POINTER(Privilege))
        checked(security.CreateRestrictedToken(original, 0, 0, None, count, deletion, 0, None, ctypes.byref(restricted)))
        if wintypes.DWORD.from_buffer(privileges(restricted)).value != 0:
            raise AssertionError("fixture token retains privileges")
        checked(security.ImpersonateLoggedOnUser(restricted))
        active = True
        yield
    finally:
        # Never leave the test runner impersonated, including assertion errors.
        if active and not security.RevertToSelf():
            os.abort()
        if restricted:
            kernel.CloseHandle(restricted)
        if original:
            kernel.CloseHandle(original)


@unittest.skipUnless(os.name == "nt", "real Windows restricted-token path fixture")
class DbgEngPathsTests(unittest.TestCase):
    def test_zero_privilege_token_can_validate_existing_local_files(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary).absolute() / "known fixture.bin"
            path.write_bytes(b"inert-known-fixture")
            with without_privileges():
                self.assertEqual(worker_dbgeng.local_path(path), path)
                self.assertEqual(path.read_bytes(), b"inert-known-fixture")
            for invalid in [r"\\server\share\file", r"C:\file:stream", r"relative\file", str(path) + "."]:
                with self.assertRaises(ValueError):
                    worker_dbgeng.local_path(invalid)
