"""Publishing inputs cannot bypass the private-metadata file policy."""

from pathlib import Path
import os
import hashlib
import tempfile
import unittest

from wukong_worker.platform.isolation import stage_file, stage_directory
from wukong_worker.artifacts.common import public_file
from wukong_worker.resources.jobs import JobManager
from wukong_worker.operations.jobs import JobService
from wukong_worker.execution.staging import executable_inputs, stage_argv


class StagedInputPolicyTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        (self.root / "files").mkdir()
        readonly = self.root / "experiment-copy"
        readonly.mkdir()
        self.experiment = {"state_root": str(self.root), "readonly": str(readonly)}
        self.private = self.root / "endpoint.json"
        self.private.write_bytes(b"controlled canary, never a real credential")

    def test_private_files_and_directory_members_are_rejected_before_copy(self):
        with self.assertRaisesRegex(ValueError, "private worker metadata"):
            stage_file(self.private, self.experiment)
        with self.assertRaisesRegex(ValueError, "private worker metadata"):
            stage_file(
                self.private,
                self.experiment,
                derived_sha256=hashlib.sha256(self.private.read_bytes()).hexdigest(),
            )
        job = self.root / "jobs/fixture"
        job.mkdir(parents=True)
        (job / "spec.json").write_bytes(b"controlled private job canary")
        (job / "output.txt").write_bytes(b"public output")
        with self.assertRaisesRegex(ValueError, "private worker metadata"):
            stage_directory(job, self.experiment, "copied-job")
        self.assertEqual(list(Path(self.experiment["readonly"]).iterdir()), [])
        with self.assertRaises(ValueError):
            stage_directory(self.root, self.experiment, "copied-state")

    def test_uploaded_metadata_like_names_are_payloads_and_remain_usable_as_inputs(self):
        seed = self.root / "files/seeds/status.json"
        seed.parent.mkdir()
        seed.write_bytes(b"ordinary seed data")
        self.assertEqual(public_file(self.root, "files/seeds/status.json"), seed)
        copied = stage_file(seed, self.experiment)
        self.assertEqual(copied.read_bytes(), seed.read_bytes())
        directory = stage_directory(seed.parent, self.experiment, "seeds")
        self.assertEqual((directory / "status.json").read_bytes(), seed.read_bytes())

    def test_interpreter_target_gets_an_executable_copy_but_data_does_not(self):
        target = self.root / "files/target"
        target.write_bytes(b"inert executable fixture")
        target.chmod(0o444)
        data = self.root / "files/data"
        data.write_bytes(b"input data")
        argv = ["python3", "files/data", "files/target"]
        selected = executable_inputs(self.root, argv, ["files/target"])
        staged = stage_argv(self.root, argv, self.experiment, selected)
        manifest = self.experiment["inputs"]
        self.assertEqual([item["access"] for item in manifest], ["read", "execute"])
        self.assertEqual(
            manifest[1]["source_sha256"], hashlib.sha256(target.read_bytes()).hexdigest()
        )
        self.assertEqual(manifest[1]["source_sha256"], manifest[1]["executed_sha256"])
        self.assertNotEqual(Path(staged[-1]), target)
        if os.name != "nt":
            self.assertEqual(Path(staged[-1]).stat().st_mode & 0o777, 0o555)
            self.assertEqual(Path(staged[1]).stat().st_mode & 0o777, 0o444)
            self.assertEqual(target.stat().st_mode & 0o777, 0o444)
        # Windows read-only files need their own cleanup permissions restored.
        for item in [target, *Path(self.experiment["readonly"]).iterdir()]:
            item.chmod(0o600)

    def test_executable_selection_cannot_publish_private_or_unlisted_files(self):
        with self.assertRaisesRegex(ValueError, "private worker metadata"):
            executable_inputs(self.root, [str(self.private)], ["endpoint.json"])
        source = self.root / "files/target"
        source.write_bytes(b"not an argument")
        with self.assertRaisesRegex(ValueError, "present in argv"):
            executable_inputs(self.root, ["python3"], ["files/target"])

    def test_private_verification_input_is_denied_before_a_job_can_be_created(self):
        target = self.root / "files/target"
        target.write_bytes(b"inert target: never executed")
        manager = JobService(
            self.root,
            trusted=True,
            isolated=False,
            jobs=JobManager(self.root, trusted=True, isolated=False),
        )
        with self.assertRaisesRegex(ValueError, "private worker metadata"):
            manager.start_job({"target": "files/target", "input": "endpoint.json"}, "replay")
        self.assertEqual(list((self.root / "jobs").iterdir()), [])

    def test_public_namespace_alias_cannot_publish_private_metadata(self):
        alias = self.root / "files/alias"
        try:
            os.symlink(self.private, alias)
        except (OSError, NotImplementedError) as error:
            self.skipTest("symlink creation unavailable: " + str(error))
        with self.assertRaisesRegex(ValueError, "private worker metadata"):
            stage_file(alias, self.experiment)
        with self.assertRaisesRegex(ValueError, "symlinks"):
            stage_directory(alias.parent, self.experiment, "aliased-inputs")
