"""Publication failures must not replace good state or authorize side effects."""
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from worker_common import atomic_json, load_json
from worker_operations import OperationLedger


class AtomicMetadataTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)

    def test_flush_failure_preserves_the_previous_complete_document(self):
        path = self.root / 'state.json'
        atomic_json(path, {'generation': 1, 'text': '原始状态'})
        previous = path.read_bytes()
        with patch('atomicwrites._proper_fsync', side_effect=OSError('injected file flush failure')):
            with self.assertRaises(OSError):
                atomic_json(path, {'generation': 2, 'text': 'replacement'})
        self.assertEqual(path.read_bytes(), previous)
        self.assertEqual(list(self.root.iterdir()), [path])

    def test_failed_reservation_flush_cannot_execute_or_replay_an_effect(self):
        ledger = OperationLedger(self.root)
        calls = []

        def execute(method, params):
            calls.append((method, params.copy()))
            return {'completed': True}

        params = {'request_id': 'trusted-flush-failure', 'action': 'continue'}
        with patch('atomicwrites._proper_fsync', side_effect=OSError('injected reservation flush failure')):
            with self.assertRaises(OSError):
                ledger.invoke('debug.control', params, execute, lambda _method, result: result)
        self.assertEqual(calls, [])
        self.assertEqual(list((self.root / 'operations').iterdir()), [])
        result = ledger.invoke('debug.control', params, execute, lambda _method, value: value)
        self.assertEqual(result, {'completed': True})
        self.assertEqual(ledger.invoke('debug.control', params, execute, lambda _method, value: value), result)
        self.assertEqual(len(calls), 1)

    def test_replacement_failure_preserves_old_bytes_and_removes_only_its_temporary(self):
        path = self.root / 'state.json'
        atomic_json(path, {'generation': 1})
        previous = path.read_bytes()
        with patch('atomicwrites.replace_atomic', side_effect=PermissionError('injected sharing violation')), \
                patch('worker_common.time.sleep'):
            with self.assertRaises(PermissionError):
                atomic_json(path, {'generation': 2})
        self.assertEqual(path.read_bytes(), previous)
        self.assertEqual(list(self.root.iterdir()), [path])

if __name__ == '__main__':
    unittest.main()
