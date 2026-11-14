"""Publishing inputs cannot bypass the private-metadata file policy."""
from pathlib import Path
import os
import hashlib
import tempfile
import unittest

from isolation import stage_file, stage_directory
from worker_common import public_file
from worker_jobs import JobManager


class StagedInputPolicyTests(unittest.TestCase):
    def setUp(self):
        self.temporary=tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root=Path(self.temporary.name).resolve()
        (self.root/'files').mkdir()
        readonly=self.root/'experiment-copy'
        readonly.mkdir()
        self.experiment={'state_root':str(self.root),'readonly':str(readonly)}
        self.private=self.root/'endpoint.json'
        self.private.write_bytes(b'controlled canary, never a real credential')

    def test_private_files_and_directory_members_are_rejected_before_copy(self):
        with self.assertRaisesRegex(ValueError,'private worker metadata'):
            stage_file(self.private,self.experiment)
        with self.assertRaisesRegex(ValueError,'private worker metadata'):
            stage_file(self.private,self.experiment,derived_sha256=hashlib.sha256(self.private.read_bytes()).hexdigest())
        job=self.root/'jobs/fixture'
        job.mkdir(parents=True)
        (job/'spec.json').write_bytes(b'controlled private job canary')
        (job/'output.txt').write_bytes(b'public output')
        with self.assertRaisesRegex(ValueError,'private worker metadata'):
            stage_directory(job,self.experiment,'copied-job')
        self.assertEqual(list(Path(self.experiment['readonly']).iterdir()),[])
        with self.assertRaises(ValueError):
            stage_directory(self.root,self.experiment,'copied-state')

    def test_uploaded_metadata_like_names_are_payloads_and_remain_usable_as_inputs(self):
        seed=self.root/'files/seeds/status.json'
        seed.parent.mkdir()
        seed.write_bytes(b'ordinary seed data')
        self.assertEqual(public_file(self.root,'files/seeds/status.json'),seed)
        copied=stage_file(seed,self.experiment)
        self.assertEqual(copied.read_bytes(),seed.read_bytes())
        directory=stage_directory(seed.parent,self.experiment,'seeds')
        self.assertEqual((directory/'status.json').read_bytes(),seed.read_bytes())

    def test_private_verification_input_is_denied_before_a_job_can_be_created(self):
        target=self.root/'files/target'
        target.write_bytes(b'inert target: never executed')
        manager=JobManager(self.root,trusted=True,isolated=False)
        with self.assertRaisesRegex(ValueError,'private worker metadata'):
            manager.start_job({'target':'files/target','input':'endpoint.json'},'replay')
        self.assertEqual(list((self.root/'jobs').iterdir()),[])

    def test_public_namespace_alias_cannot_publish_private_metadata(self):
        alias=self.root/'files/alias'
        try:
            os.symlink(self.private,alias)
        except (OSError,NotImplementedError) as error:
            self.skipTest('symlink creation unavailable: '+str(error))
        with self.assertRaisesRegex(ValueError,'private worker metadata'):
            stage_file(alias,self.experiment)
        with self.assertRaisesRegex(ValueError,'symlinks'):
            stage_directory(alias.parent,self.experiment,'aliased-inputs')
