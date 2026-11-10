"""Regression checks for crash evidence, not an exploit evaluation suite."""
import base64
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
from worker_common import atomic_json, load_json
from worker_crash import CrashService, location_signature, normalize_address
from worker_job_supervisor import supervise_job


class CrashTests(unittest.TestCase):
    def test_signature_tracks_code_identity_and_offset_not_aslr(self):
        def snapshot(base, offset=16, digest="one"):
            profile = {"maps":[{"start":base,"end":base+4096,"offset":4096,"inode":3,"device":[8,1]}],
                       "modules":[{"inode":3,"device":[8,1],"sha256":digest}]}
            return {"signal":"SIGSEGV", "pc_location":normalize_address(base+offset,profile)}
        self.assertEqual(location_signature(snapshot(4096)),location_signature(snapshot(8192)))
        self.assertNotEqual(location_signature(snapshot(4096)),location_signature(snapshot(4096,17)))
        self.assertNotEqual(location_signature(snapshot(4096)),location_signature(snapshot(4096,digest="two")))
        self.assertIsNone(location_signature(snapshot(4096,digest=None)))

    def test_exact_pattern_width_required(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory); (root/"pattern").write_bytes(b"aaaabaaacaaa")
            with self.assertRaisesRegex(ValueError,"exactly n"):
                CrashService(root,True,False).dispatch("cyclic.offset",{"pattern":"pattern",
                    "observed_base64":base64.b64encode(b"aaaabaaa").decode(),"alphabet":"abcd","n":4})

    def minimize(self, strong, weak=False):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory).resolve(); job=root/"job"; job.mkdir()
            (root/"input").write_bytes(b"xxAyy")
            atomic_json(job/"spec.json",{"kind":"minimize","runner_token":"test","input":"input",
                "max_attempts":40,"minimize_timeout_secs":5,"allow_signal_only":weak})
            atomic_json(job/"status.json",{})
            def replay(root,directory,spec,data,index):
                result={"state":"completed","signal":11,"exit_code":-11}
                if strong: result["location_signature"]="location:"+("A" if b"A" in data else "other")
                return result
            with patch("worker_job_supervisor.replay",side_effect=replay): supervise_job(root,job)
            status=load_json(job/"status.json")
            return status, (job/"minimized.bin").read_bytes() if (job/"minimized.bin").exists() else None

    def test_minimize_rejects_same_signal_different_location(self):
        status,data=self.minimize(True)
        self.assertEqual(status["state"],"completed")
        self.assertEqual(data,b"A")
        self.assertEqual(status["result"]["signature_strength"],"location_and_frames")

    def test_minimize_refuses_implicit_weak_equivalence(self):
        status,data=self.minimize(False)
        self.assertEqual(status["state"],"failed")
        self.assertIn("location signature unavailable",status["error"])
        self.assertIsNone(data)

    def test_weak_minimization_is_labelled(self):
        status,_=self.minimize(False,True)
        self.assertEqual(status["result"]["signature_strength"],"signal_only")

if __name__ == "__main__": unittest.main()
