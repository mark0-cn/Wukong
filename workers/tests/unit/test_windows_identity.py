"""An OS boot identity must change with loader sequence, not wall-clock time."""
import struct
import unittest
import uuid

from worker_windows_identity import SHARED_PREFIX_BYTES, _decode_boot_identity


class NativeBootIdentityTests(unittest.TestCase):
    def records(self, sequence=7, major=10, build=22631, architecture=9):
        environment=bytearray(32)
        environment[:16]=uuid.UUID('12345678-1234-5678-9abc-def012345678').bytes_le
        struct.pack_into('<I',environment,16,2)
        shared=bytearray(SHARED_PREFIX_BYTES)
        struct.pack_into('<I',shared,0x260,build)
        struct.pack_into('<H',shared,0x26A,architecture)
        struct.pack_into('<I',shared,0x26C,major)
        struct.pack_into('<I',shared,0x2C4,sequence)
        return environment,shared

    def test_loader_sequence_distinguishes_reboots_with_the_same_boot_entry(self):
        environment,shared=self.records()
        first=_decode_boot_identity(environment,shared)
        self.assertEqual(first,'windows:12345678123456789abcdef012345678:7')
        environment,next_boot=self.records(sequence=8)
        self.assertNotEqual(first,_decode_boot_identity(environment,next_boot))
        # Changing the shared SystemTime/InterruptTime cannot fabricate a reboot.
        shared[8:32]=b'\xff'*24
        self.assertEqual(first,_decode_boot_identity(environment,shared))

    def test_unsupported_layout_missing_identity_and_short_buffers_fail_closed(self):
        environment,shared=self.records()
        cases=[(environment[:-1],shared),(environment,shared[:-1]),(bytearray(32),shared),
               self.records(major=9),self.records(build=7601),self.records(architecture=12)]
        for environment,shared in cases:
            with self.subTest(size=len(shared)),self.assertRaises(ValueError):
                _decode_boot_identity(environment,shared)
