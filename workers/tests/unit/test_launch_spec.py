"""Both execution modes reject the same changed and escaping launch members."""
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from worker_launch import load
from worker_network import network_profile


class SharedLaunchTests(unittest.TestCase):
    def test_local_and_network_share_artifact_validation(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            base = root / "files/challenge"
            base.mkdir(parents=True)
            target = base / "target"
            target.write_bytes(b"known fixture")
            spec = {"schema_version": 1, "challenge_id": "fixture", "base": "files/challenge",
                    "target": "target", "files": {"target": hashlib.sha256(target.read_bytes()).hexdigest()},
                    "args": [], "cwd": ".", "input_mode": "stdin_pipe", "env": {},
                    "service": {"mode": "local_service", "port": 1337}}
            params = {"launch_spec": "files/challenge/launch.json", "target": "files/challenge/target"}
            launch = base / "launch.json"
            launch.write_text(json.dumps(spec))
            for read in (load, network_profile):
                read(root, params)
            target.write_bytes(b"changed")
            for read in (load, network_profile):
                with self.assertRaisesRegex(ValueError, "input changed"):
                    read(root, params)
            target.write_bytes(b"known fixture")
            spec["files"]["../outside"] = "unused"
            launch.write_text(json.dumps(spec))
            for read in (load, network_profile):
                with self.assertRaisesRegex(ValueError, "invalid launch member"):
                    read(root, params)


if __name__ == "__main__": unittest.main()
