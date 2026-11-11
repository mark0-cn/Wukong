"""Keep fixed native loader failures useful without changing global os.name."""
import subprocess
import tempfile
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

import worker_dbgeng


class DbgEngDiagnosticsTests(unittest.TestCase):
    def test_failed_fixed_probe_retains_bounded_loader_reason(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'windows.backend.json').write_text('{}', encoding='utf-8')
            process = Mock(return_value=subprocess.CompletedProcess([], 1, b'', b'x'*2048 + b' loader access denied (WinError 5)'))
            module = SimpleNamespace(run=process, TimeoutExpired=subprocess.TimeoutExpired)
            with patch.object(worker_dbgeng, 'os', SimpleNamespace(name='nt')), \
                    patch.object(worker_dbgeng, 'profile', return_value=(root/'helper.exe', root, root, {})), \
                    patch.object(worker_dbgeng, 'subprocess', module):
                result = worker_dbgeng.doctor(root, trusted=True)
            self.assertFalse(result['available'])
            self.assertIn('loader access denied (WinError 5)', result['reason'])
            self.assertLessEqual(len(result['reason']), 512)
            self.assertIn(b'"operation":"doctor"', process.call_args.kwargs['input'])
