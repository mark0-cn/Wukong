"""A boot epoch must come from the System record, never the current daemon."""
import ctypes
import unittest

from worker_windows_identity import _ProcessPrefix, _process_boot_identity


class NativeProcessInventoryTests(unittest.TestCase):
    def record(self, pid, created, next_offset=0):
        record=_ProcessPrefix()
        record.pid=pid
        record.created=created
        record.next_offset=next_offset
        # A forged name pointer must never be followed by the bounded parser.
        record.name_buffer=1
        return bytes(record)

    def test_selects_the_system_epoch_independently_of_daemon_pid_and_name_pointer(self):
        size=ctypes.sizeof(_ProcessPrefix)
        data=self.record(12345,999,size)+self.record(4,123456789012345678)
        self.assertEqual(_process_boot_identity(data),'windows:123456789012345678')
        self.assertEqual(_ProcessPrefix.created.offset,32)
        self.assertIn(_ProcessPrefix.pid.offset,{68,80})

    def test_missing_system_zero_epoch_and_invalid_record_extents_fail_closed(self):
        size=ctypes.sizeof(_ProcessPrefix)
        for data in [b'',self.record(4,0),self.record(1,123),
                     self.record(1,123,1),self.record(1,123,size+1),
                     self.record(4,123,size+1),self.record(4,123)[:-1]]:
            with self.subTest(data=data[:8]), self.assertRaises(ValueError):
                _process_boot_identity(data)
