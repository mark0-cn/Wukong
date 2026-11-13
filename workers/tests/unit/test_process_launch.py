"""A process job uses the shared LaunchSpec without per-call launch overrides."""
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch, Mock
from worker_jobs import JobManager

class ProcessLaunchPlanningTests(unittest.TestCase):
    def test_launch_only_job_records_manifest_timeout_and_rejects_overrides_before_spawn(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary);base=root/'files/case';base.mkdir(parents=True)
            (base/'target').write_bytes(b'inert fixture, never executed')
            (base/'input.bin').write_bytes(b'data')
            spec=dict(schema_version=1,base='files/case',target='target',files={'target':hashlib.sha256((base/'target').read_bytes()).hexdigest()},args=['@@'],cwd='.',input_mode='argv_file',env={},timeout_secs=5)
            (base/'launch.json').write_text(json.dumps(spec))
            manager=JobManager(root,True,False)
            params=dict(launch_spec='files/case/launch.json',input='files/case/input.bin')
            with patch('worker_jobs.subprocess.Popen',return_value=Mock(pid=123)) as spawn:
                result=manager.start_job(params)
                saved=json.loads((root/'jobs'/result['job_id']/'spec.json').read_text())
                self.assertEqual(saved['timeout_secs'],5)
                self.assertEqual(saved['launch_spec'],params['launch_spec'])
                self.assertEqual(spawn.call_count,1)
                for override in [dict(argv=['other']),dict(env={'AFL_X':'1'}),dict(cwd='files/case'),dict(timeout_secs=6)]:
                    with self.assertRaisesRegex(ValueError,'LaunchSpec owns'):
                        manager.start_job({**params,**override})
                self.assertEqual(spawn.call_count,1)

if __name__=='__main__': unittest.main()
