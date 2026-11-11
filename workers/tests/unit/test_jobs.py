"""Job planning keeps optional request identities optional without executing targets."""
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from worker_jobs import JobManager


class FuzzPlanningTests(unittest.TestCase):
    def test_missing_request_id_allocates_distinct_campaigns_and_explicit_ids_remain_stable(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary)
            (root/'files/seeds').mkdir(parents=True)
            (root/'files/fixture').write_bytes(b'trusted inert fixture: never executed')
            (root/'files/seeds/seed').write_bytes(b'input')
            manager=JobManager(root,trusted=True,isolated=False)
            params={'target':'files/fixture','seeds':'files/seeds'}
            with patch.object(manager,'doctor',return_value={'capabilities':{'fuzz_qemu':True}}), \
                    patch.object(manager,'start_job',side_effect=lambda spec,kind:spec):
                first=manager.dispatch('fuzz.start',params)
                second=manager.dispatch('fuzz.start',params)
                self.assertNotEqual(first['afl_output'],second['afl_output'])
                stable=manager.dispatch('fuzz.start',{**params,'request_id':'stable-campaign'})
                self.assertEqual(stable['afl_output'],manager.dispatch('fuzz.start',{**params,'request_id':'stable-campaign'})['afl_output'])
            for spec in [first,second,stable]:
                self.assertTrue((root/spec['afl_output']).is_dir())
                self.assertEqual(spec['argv'][:2],['afl-fuzz','-Q'])
