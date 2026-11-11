"""Functional LaunchSpec tests using trusted, freshly compiled fixtures."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import unittest
from workbench_prepare import prepare
from worker_launch import load, resolve, digest

@unittest.skipUnless(os.name=="posix" and shutil.which("gcc"),"Linux compiler required")
class LaunchTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory(); self.root=Path(self.temp.name)
        self.base=self.root/"files/challenge"; (self.base/"original").mkdir(parents=True)
        (self.base/"prepared").mkdir()
        source=self.base/"original/probe.c"
        source.write_text('#include <stdio.h>\n#include <stdlib.h>\nint main(int argc,char **argv){char b[64]; FILE *f=fopen("marker.txt","r"); if(!f)return 3; fclose(f); FILE *input=argc>1?fopen(argv[1],"r"):stdin; if(!input||!fgets(b,sizeof b,input))return 4; printf("%s:%s",getenv("CHECK_VALUE"),b); return 0;}\n')
        subprocess.run(["gcc","-g","-o",str(self.base/"original/probe"),str(source)],check=True)
        (self.base/"original/marker.txt").write_text("present")
        shutil.copyfile("/lib/x86_64-linux-gnu/libc.so.6",self.base/"original/libc.so.6")
        shutil.copyfile("/lib64/ld-linux-x86-64.so.2",self.base/"original/ld.so")
        self.manifest={"id":"probe","target":{"binary":"original/probe"},"runtime":{"kind":"loader","libc":"original/libc.so.6","loader":"original/ld.so"},"launch":{"argv":[],"cwd":"original","input":"stdin_pipe","env":{"CHECK_VALUE":"expected"},"timeout_seconds":5},"network":{"mode":"offline"}}
        (self.base/"launch-request.json").write_text(json.dumps(self.manifest)); prepare(self.base)
        self.params={"target":"files/challenge/original/probe","launch_spec":"files/challenge/launch.json"}
        self.isolated=os.geteuid()==0
    def tearDown(self): self.temp.cleanup()
    @unittest.skipUnless(hasattr(os,"geteuid") and os.geteuid()==0,"requires the trusted root supervisor")
    def test_prepare_imported_files_owned_by_another_uid(self):
        paths=[self.base/"original/probe",self.base/"prepared/libraries/libc.so.6",self.base/"prepared/libraries/ld.so"]
        for path in paths: os.chown(path,20000,20000)
        # check-workbench runs this with CHOWN and without FOWNER, like the app.
        prepare(self.base)
        prepare(self.base)
        for path in paths:
            self.assertEqual(path.stat().st_uid,0)
            self.assertEqual(path.stat().st_mode & 0o777,0o555)
    def test_interaction_and_replay_share_loader_cwd_and_env(self):
        from worker_interactive import InteractSession
        session=InteractSession(self.root,{**self.params,"argv":["unused"]},self.isolated)
        try:
            result=session.write(b"hello\n",5)
            self.assertIn("expected:hello",result["captured"])
        finally: session.close()
        from worker_job_supervisor import replay
        # Match the real supervisor's public artifact namespace. Root-level
        # scratch paths deliberately remain private to the worker.
        trials=self.root/"jobs/replay"; trials.mkdir(parents=True)
        result=replay(self.root,trials,{**self.params,"isolated":self.isolated,"input_mode":"stdin","timeout_secs":5,"output_limit":4096,"kind":"replay"},b"hello\n",0)
        self.assertEqual(result["exit_code"],0)
        self.assertIn("expected:hello",result["stdout"])
        self.assertTrue(result["launch_identity"])
    def test_argv_file_interaction_and_replay_use_staged_input(self):
        from worker_interactive import InteractSession
        from worker_job_supervisor import replay
        spec=json.loads((self.base/"launch.json").read_text())
        spec.update(input_mode="argv_file",args=["@@"])
        (self.base/"launch.json").write_text(json.dumps(spec))
        payload=self.root/"files/input.bin";payload.write_bytes(b"from-file\n")
        session=InteractSession(self.root,{**self.params,"argv":["unused"],"input":"files/input.bin"},self.isolated)
        try:
            self.assertIn("expected:from-file",session.write(b"",5)["captured"])
        finally: session.close()
        trials=self.root/"jobs/argv-replay";trials.mkdir(parents=True)
        result=replay(self.root,trials,{**self.params,"isolated":self.isolated,"input_mode":"argv","timeout_secs":5,"output_limit":4096,"kind":"replay"},b"from-replay\n",0)
        self.assertEqual(result["exit_code"],0)
        self.assertIn("expected:from-replay",result["stdout"])
        self.assertTrue(result["launch_identity"])
    def test_debugger_uses_declared_libc(self):
        from worker_debugger import DebugSession
        session=DebugSession(self.root,self.params,self.isolated)
        try:
            self.assertNotEqual(session.command("-break-insert main",5)["class"],"error")
            self.assertNotEqual(session.command("-exec-run",5)["class"],"error")
            deadline=time.monotonic()+5
            while time.monotonic()<deadline:
                if any(e.get("message")=="stopped" for e in session.events): break
                time.sleep(.05)
            profile=session.profile()["process"]
            self.assertTrue(any(m.get("sha256")==digest(self.base/"prepared/libraries/libc.so.6") for m in profile["modules"]),json.dumps(profile["modules"]))
        finally: session.close()
    def test_changed_libc_and_target_are_rejected(self):
        spec=json.loads((self.base/"launch.json").read_text());spec["files"]["prepared/libraries/libc.so.6"]="0"*64
        (self.base/"launch.json").write_text(json.dumps(spec))
        with self.assertRaisesRegex(ValueError,"launch input changed"):load(self.root,self.params)
    def test_pty_interaction(self):
        from worker_interactive import InteractSession
        spec=json.loads((self.base/"launch.json").read_text());spec["input_mode"]="pty"
        (self.base/"launch.json").write_text(json.dumps(spec))
        session=InteractSession(self.root,{**self.params,"argv":["unused"]},self.isolated)
        try: self.assertIn("expected:hello",session.write(b"hello\n",5)["captured"])
        finally: session.close()

if __name__=="__main__":unittest.main()
