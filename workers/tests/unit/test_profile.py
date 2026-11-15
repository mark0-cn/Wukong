"""Functional checks for runtime identities, not pwn benchmarks."""
import hashlib
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
from worker_profile import file_identity, parse_maps, ProfileService

class ProfileTests(unittest.TestCase):
    def test_maps_preserve_permissions_offsets_and_spaces(self):
        rows = parse_maps("1000-2000 r-xp 00001000 08:01 42 /tmp/a b\n2000-3000 rw-p 0 00:00 0 [heap]\n")
        self.assertEqual(rows[0]["offset"],4096)
        self.assertEqual(rows[0]["path"],"/tmp/a b")
        self.assertEqual(rows[1]["permissions"],"rw-p")
    def test_identity_never_claims_mapping(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory); (root/"target").write_bytes(b"fixture")
            identity=file_identity(root/"target")
            self.assertEqual(identity["sha256"],hashlib.sha256(b"fixture").hexdigest())
            service=ProfileService(root,True,False)
            self.assertFalse(service.profile({"target":"target"})["mapped_match"])
            with self.assertRaises(ValueError): service.profile({"target":"../outside"})
    def test_bundle_copies_preserve_original_and_replay(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)
            for key in ["target","libc","loader"]: (root/key).write_bytes(key.encode())
            def fake(path):
                data=Path(path).read_bytes()
                return {"path":str(path),"sha256":hashlib.sha256(data).hexdigest(),"architecture":"x64","bits":64}
            service=ProfileService(root,True,False)
            with patch("worker_profile.file_identity",side_effect=fake):
                result=service.prepare({"target":"target","libc":"libc","loader":"loader"})
                self.assertEqual(result,service.prepare({"target":"target","libc":"libc","loader":"loader"}))
            self.assertEqual((root/"target").read_bytes(),b"target")
            self.assertEqual(len(result["artifacts"]),3)
            # Interactive sessions launch in fresh directories; argv must not
            # depend on the current directory even on trusted-local workers.
            for index in [0,2,3]: self.assertTrue(Path(result["launch_argv"][index]).is_absolute())
            self.assertEqual((root/result["launch_argv"][2]/"libc.so.6").read_bytes(),b"libc")

if __name__ == "__main__": unittest.main()
