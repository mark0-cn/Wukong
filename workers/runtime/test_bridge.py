"""Portable runtime transport tests; all executed commands are trusted fixtures."""
import json
import os
from pathlib import Path
import sys
import signal
import tempfile
import time
import unittest
from unittest.mock import patch
from integration_test import Client


class SessionLaunchTests(unittest.TestCase):
    def test_lost_session_error_kind_survives_durable_replay(self):
        import worker_runtime as bridge
        from worker_common import WorkerError,rpc_error
        with tempfile.TemporaryDirectory() as directory:
            runtime = bridge.Runtime(Path(directory), trusted=True)
            for _ in range(2):
                with self.assertRaises(WorkerError) as error:
                    runtime.dispatch("debug.close", {"session_id": "missing", "request_id": "lost-debug"})
                self.assertEqual(rpc_error(error.exception)["data"]["kind"], "session_lost")

    def test_isolated_interactive_launch_drops_credentials(self):
        import worker_interactive as bridge
        import isolation
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            work = root / "work"
            work.mkdir()
            experiment = {"uid": 20000, "root": str(root), "readonly": str(root), "work": str(work)}
            with patch.object(isolation, "allocate_uid", return_value=20000), \
                    patch.object(isolation, "sandbox", return_value=experiment), \
                    patch.object(isolation, "seal"), \
                    patch.object(bridge.subprocess, "Popen", side_effect=RuntimeError("capture options")) as launch:
                with self.assertRaisesRegex(RuntimeError, "capture options"):
                    bridge.InteractSession(root, {"argv": [sys.executable, "-c", "pass"]}, isolated=True)
                options = launch.call_args.kwargs
                self.assertEqual(options["user"], 20000)
                self.assertEqual(options["group"], 20000)
                self.assertEqual(options["extra_groups"], [])
                self.assertEqual(options["env"]["HOME"], str(work))
                self.assertEqual(options["env"]["TMPDIR"], str(work))


class GdbMiTests(unittest.TestCase):
    def session(self, directory, output):
        import worker_debugger as bridge
        import collections
        import io
        import queue
        import threading
        from types import SimpleNamespace
        session = bridge.DebugSession.__new__(bridge.DebugSession)
        session.id = "mi-fixture"
        session.directory = Path(directory)
        session.transcript_bytes = 0
        session.dead = False
        session.token = 0
        session.isolated = False
        session.lock = threading.Lock()
        session.records = queue.Queue(maxsize=8)
        session.events = collections.deque(maxlen=256)
        session.process = SimpleNamespace(stdout=io.StringIO(output), stdin=io.StringIO(), poll=lambda: None)
        return session

    def test_nested_payloads_escaping_and_async_events(self):
        records = r'''*stopped,reason="breakpoint-hit",frame={func="main",line="12"}
1^done,bkpt={number="1",fullname="a\"b.c"},stack=[frame={level="0",func="main"}]
(gdb)
'''
        with tempfile.TemporaryDirectory() as directory:
            session = self.session(directory, records)
            session.read_records()
            session.dead = False
            result = session.command("-stack-list-frames", 1)
            self.assertEqual(result["class"], "done")
            self.assertEqual(result["payload"]["bkpt"]["fullname"], 'a"b.c')
            self.assertEqual(result["payload"]["stack"][0]["func"], "main")
            self.assertEqual(result["events"][0]["message"], "stopped")
            self.assertEqual(result["events"][0]["payload"]["frame"]["line"], "12")
            self.assertEqual((Path(directory) / "transcript.mi").read_text(), records)

    def test_oversized_mi_records_invalidate_the_session(self):
        import worker_debugger as bridge
        with tempfile.TemporaryDirectory() as directory:
            session = self.session(directory, '1^done,value="' + 'x' * 70000 + '"\n')
            with patch.object(bridge, "terminate_tree") as terminate:
                session.read_records()
            terminate.assert_called_once_with(session.process)
            self.assertTrue(session.dead)
            self.assertTrue(session.records.empty())


class GhidraProjectLockTests(unittest.TestCase):
    def test_unknown_analyzer_claim_prevents_reentry_after_lock_release(self):
        import ghidra_backend
        import hashlib
        from filelock import FileLock, Timeout
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / "files/sample"
            target.parent.mkdir()
            target.write_bytes(b"fixture")
            installation = root / "installation"
            headless = installation / "support/analyzeHeadless"
            headless.parent.mkdir(parents=True)
            headless.write_text("fixture launcher")
            properties = installation / "Ghidra/application.properties"
            properties.parent.mkdir()
            properties.write_text("application.version=fixture")
            scripts = root / "scripts"
            scripts.mkdir()
            (scripts / "ReAgentBridge.java").write_text("fixture bridge")
            params = {"target": "files/sample", "target_sha256": hashlib.sha256(b"fixture").hexdigest(), "query": {"operation": "functions"}}
            with patch.dict(os.environ, {"RE_AGENT_GHIDRA_HEADLESS": str(headless), "RE_AGENT_GHIDRA_SCRIPT_DIR": str(scripts)}), \
                    patch.object(ghidra_backend, "run_process", side_effect=RuntimeError("owner lost")) as execute:
                with self.assertRaisesRegex(RuntimeError, "owner lost"):
                    ghidra_backend.query(root, params)
                project = next((root / "ghidra/projects").iterdir())
                self.assertTrue((project / "analysis.active.json").exists())
                with self.assertRaisesRegex(ValueError, "outcome is unknown"):
                    ghidra_backend.query(root, params)
                self.assertEqual(execute.call_count, 1)
                # The kernel lock is released, but the uncertain analyzer claim remains.
                with FileLock(str(project / "worker.lock"), timeout=0):
                    with self.assertRaises(Timeout):
                        FileLock(str(project / "worker.lock"), timeout=0).acquire()
                (project / "analysis.active.json").unlink()
                with ghidra_backend.analysis_claim(project, ["fixture"]):
                    self.assertTrue((project / "analysis.active.json").exists())
                self.assertFalse((project / "analysis.active.json").exists())


class RuntimeTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.client = Client(self.root, trusted_local=True, environment={**os.environ, "OPENAI_API_KEY": "fixture-secret", "ANTHROPIC_API_KEY": "fixture-secret"})

    def tearDown(self):
        try:
            self.client.call("worker.shutdown")
        finally:
            self.client.close()
            # Windows may still have the final shutdown frame/log open briefly.
            for _ in range(30):
                try:
                    self.temporary.cleanup()
                    break
                except PermissionError:
                    time.sleep(0.05)

    def test_reconnect_and_request_identity(self):
        params = {"argv": [sys.executable, "-c", "import time;time.sleep(0.5);print('durable')"], "request_id": "durable-1", "timeout_secs": 4}
        job = self.client.call("job.start", **params)
        self.client.close()
        self.client = Client(self.root, trusted_local=True)
        duplicate = self.client.call("job.start", **params)
        self.assertEqual(job["job_id"], duplicate["job_id"])
        result = self.client.finish(job)
        self.assertEqual(result["state"], "completed")
        self.assertIn("durable", result["result"]["stdout"])
        self.assertEqual(len(self.client.call("job.list")), 1)

    def test_output_cap_and_timeout(self):
        result = self.client.finish(self.client.call("job.start", argv=[sys.executable, "-c", "print('a'*200000)"], output_limit=1024))
        self.assertTrue(result["result"]["output_truncated"])
        self.assertLessEqual(len(result["result"]["stdout"]), 1024)
        result = self.client.finish(self.client.call("job.start", argv=[sys.executable, "-c", "import time;time.sleep(60)"], timeout_secs=1))
        self.assertEqual(result["state"], "timed_out")

    def test_exec_run_sync_and_idempotent_replay(self):
        result = self.client.call("exec.run", command="echo exec-ok", request_id="exec-1")
        self.assertEqual(result["exit_code"], 0)
        self.assertIn("exec-ok", result["stdout"])
        replay = self.client.call("exec.run", command="echo exec-ok", request_id="exec-1")
        self.assertEqual(replay, result, "identical exec replay must return the recorded result")
        with self.assertRaises(AssertionError):
            self.client.call("exec.run", command="", request_id="exec-3")

    @unittest.skipIf(os.name == "nt", "POSIX shell fixture")
    def test_exec_run_timeout_kills_the_process(self):
        timed = self.client.call("exec.run", command="sleep 60", timeout_secs=1, request_id="exec-2")
        self.assertEqual(timed["state"], "timed_out")

    @unittest.skipIf(os.name == "nt", "POSIX shell pipeline fixture")
    def test_exec_run_pipes_payload_to_stdin(self):
        command = "printf 'payload-bytes' | python3 -c 'import sys; print(sys.stdin.read().strip()+chr(45)+chr(101)+chr(99)+chr(104)+chr(111))'"
        result = self.client.call("exec.run", command=command, request_id="exec-pipe")
        self.assertEqual(result["exit_code"], 0)
        self.assertIn("payload-bytes-echo", result["stdout"])

    @unittest.skipIf(os.name == "nt", "executable script fixture")
    def test_poc_verify_stdin_mode_pipes_payload_bytes(self):
        script = "#!/bin/sh\npython3 -c 'import sys; print(\"got\", len(sys.stdin.buffer.read()), flush=True)'\n"
        self.client.put("stdin_target.sh", script.encode(), executable=True)
        self.client.put("payload.bin", b"A" * 40 + b"B" * 8)
        job = self.client.call(
            "poc.verify",
            target="files/stdin_target.sh",
            input="files/payload.bin",
            input_mode="stdin",
            args=[],
            trials=1,
            expected={"stdout_contains": "got 48"},
            request_id="poc-stdin",
            verifier_agent_id="verifier-fixture",
            producer_agent_id="producer-fixture",
            call_id="stdin-call",
        )
        result = self.client.finish(job)
        self.assertEqual(result["state"], "completed")
        self.assertTrue(result["result"]["passed"], result["result"])
        self.assertEqual(result["result"]["results"][0]["input_mode"], "stdin")

    def test_supervisor_survives_owner_daemon_restart(self):
        job = self.client.call("job.start", argv=[sys.executable, "-c", "import time;time.sleep(1);print('owner-recovered')"], request_id="owner-recovery")
        daemon_pid = self.client.call("ping")["daemon_pid"]
        self.client.close()
        os.kill(daemon_pid, signal.SIGTERM)
        self.client = Client(self.root, trusted_local=True)
        result = self.client.finish(job)
        self.assertEqual(result["state"], "completed")
        self.assertIn("owner-recovered", result["result"]["stdout"])

    def test_cancel_kills_descendants(self):
        marker = self.root / "child-survived.txt"
        child = "import time,pathlib;time.sleep(2);pathlib.Path(" + repr(str(marker)) + ").write_text('alive')"
        command = "import subprocess,sys,time;subprocess.Popen([sys.executable,'-c'," + repr(child) + "]);time.sleep(60)"
        job = self.client.call("job.start", argv=[sys.executable, "-c", command], timeout_secs=10)
        time.sleep(0.3)
        self.client.call("job.cancel", job_id=job["job_id"], request_id="cancel-1")
        self.assertEqual(self.client.finish(job)["state"], "cancelled")
        time.sleep(2.1)
        self.assertFalse(marker.exists(), "cancelled command left a descendant running")

    def test_worker_receives_no_provider_credentials(self):
        result = self.client.finish(self.client.call("job.start", argv=[sys.executable, "-c", "import os;print(','.join(k for k in os.environ if 'API_KEY' in k or 'TOKEN' in k))"]))
        self.assertEqual(result["result"]["stdout"].strip(), "")
        self.assertEqual(self.client.call("doctor")["protocol"], 1)

    def test_job_environment_carries_state_and_job_dirs(self):
        result = self.client.finish(self.client.call("job.start", argv=[sys.executable, "-c", "import os;print(os.environ['RE_AGENT_STATE_DIR']);print(os.environ['RE_AGENT_JOB_DIR'])"]))
        state_dir, job_dir = result["result"]["stdout"].strip().splitlines()
        self.assertTrue(Path(state_dir).is_dir())
        self.assertTrue(Path(job_dir).is_dir())
        self.assertEqual(Path(state_dir).resolve(), self.root.resolve())

    def test_artifact_confinement(self):
        self.client.put("example.txt", b"example")
        # Client.call asserts worker errors, which is intentional in these cases.
        with self.assertRaises(AssertionError):
            self.client.call("file.get", path="../outside")
        with self.assertRaises(AssertionError):
            self.client.call("file.get", path="endpoint.json")
        with self.assertRaises(AssertionError):
            self.client.call("file.put", path="../../outside", data_base64="eA==")
        with self.assertRaises(AssertionError):
            self.client.call("job.start", argv=[sys.executable, "-c", "pass"], env={"OPENAI_API_KEY": "rejected"})

    def test_interactive_session_echo_and_ownership(self):
        program = "import sys\nfor line in sys.stdin:\n    line=line.strip()\n    if line=='quit': break\n    print('echo:'+line, flush=True)\n"
        started = self.client.call("interact.start", argv=[sys.executable, "-c", program])
        session_id = started["session_id"]
        self.assertEqual(started["state"], "running")
        first = self.client.call("interact.write", session_id=session_id, data_base64="aGVsbG8K", read_timeout_secs=5)
        self.assertIn("echo:hello", first["captured"])
        self.assertEqual(first["state"], "running")
        second = self.client.call("interact.write", session_id=session_id, data_base64="cXVpdAo=", read_timeout_secs=5)
        self.assertEqual(second["state"], "exited")
        self.assertEqual(second["exit_code"], 0)
        closed = self.client.call("interact.close", session_id=session_id)
        self.assertEqual(closed["state"], "closed")
        with self.assertRaises(AssertionError):
            self.client.call("interact.write", session_id=session_id, data_base64="eA==")
        with self.assertRaises(AssertionError):
            self.client.call("interact.write", session_id="absent", data_base64="eA==")

    def test_interactive_writes_are_ledger_deduplicated(self):
        program = "import sys\nfor line in sys.stdin:\n    line=line.strip()\n    if line=='quit': break\n    print('echo:'+line, flush=True)\n"
        started = self.client.call("interact.start", argv=[sys.executable, "-c", program], request_id="interact-start-dedup")
        session_id = started["session_id"]
        first = self.client.call("interact.write", session_id=session_id, data_base64="aGVsbG8K", read_timeout_secs=5, request_id="write-dedup-1")
        self.assertIn("echo:hello", first["captured"])
        # An identical replay must return the recorded result instead of
        # writing the bytes a second time.
        replay = self.client.call("interact.write", session_id=session_id, data_base64="aGVsbG8K", read_timeout_secs=5, request_id="write-dedup-1")
        self.assertEqual(first, replay)
        self.assertEqual(replay["captured"].count("echo:hello"), 1)
        # A completed start replay returns the same session instead of
        # launching a second process.
        started_replay = self.client.call("interact.start", argv=[sys.executable, "-c", program], request_id="interact-start-dedup")
        self.assertEqual(started_replay["session_id"], session_id)

    @unittest.skipIf(os.name == "nt", "POSIX process group regression")
    def test_interactive_close_reaps_children_after_leader_exits(self):
        marker = self.root / "orphan-survived.txt"
        child = "import pathlib,time;time.sleep(1.5);pathlib.Path(" + repr(str(marker)) + ").write_text('alive')"
        program = "import subprocess,sys;subprocess.Popen([sys.executable,'-c'," + repr(child) + "]);print('spawned',flush=True)"
        started = self.client.call("interact.start", argv=[sys.executable, "-c", program])
        time.sleep(0.25)
        result = self.client.call("interact.write", session_id=started["session_id"], data_base64="", read_timeout_secs=1)
        self.assertEqual(result["state"], "exited")
        self.assertFalse(result["data_sent"])
        self.client.call("interact.close", session_id=started["session_id"])
        time.sleep(1.5)
        self.assertFalse(marker.exists(), "closing an exited leader left its child running")

    def test_angr_solve_validates_parameters(self):
        self.client.put("dummy.bin", b"not-an-elf")
        with self.assertRaises(AssertionError):
            self.client.call("angr.solve", target="files/missing.bin", find=[4194304])
        with self.assertRaises(AssertionError):
            self.client.call("angr.solve", target="files/dummy.bin", find=[])
        with self.assertRaises(AssertionError):
            self.client.call("angr.solve", target="files/dummy.bin", find=[4194304], stdin_length=999999)
        with self.assertRaises(AssertionError):
            self.client.call("angr.solve", target="files/dummy.bin", find=[4194304], avoid=["not-an-int"])

    def test_parse_address_accepts_integer_and_hex_spellings(self):
        from worker_common import parse_address
        self.assertEqual(parse_address(0), 0)
        self.assertEqual(parse_address(4198400), 4198400)
        self.assertEqual(parse_address("4198400"), 4198400)
        self.assertEqual(parse_address("0x401000"), 0x401000)
        self.assertEqual(parse_address(" 0X401000 "), 0x401000)
        self.assertEqual(parse_address("1_000"), 1000)
        for invalid in ["-1", "0x", "nope", "", 2**64, 1.5, True, None, [1]]:
            with self.assertRaises(ValueError, msg=repr(invalid)):
                parse_address(invalid)




class OperationLedgerTests(unittest.TestCase):
    def test_completed_effect_replays_after_ledger_reconstruction(self):
        from worker_operations import OperationLedger
        with tempfile.TemporaryDirectory() as directory:
            calls = []
            def execute(method, params):
                calls.append(params)
                return {"value": "committed"}
            params = {"request_id": "stable", "command": "fixture"}
            reconcile = lambda method, result: result
            first = OperationLedger(Path(directory)).invoke("exec.run", params, execute, reconcile)
            replay = OperationLedger(Path(directory)).invoke("exec.run", params, execute, reconcile)
            self.assertEqual(first, replay)
            self.assertEqual(len(calls), 1)
            with self.assertRaisesRegex(ValueError, "different operation"):
                OperationLedger(Path(directory)).invoke("exec.run", {**params, "command": "changed"}, execute, reconcile)

    def test_owner_death_does_not_repeat_an_unknown_interactive_write(self):
        from worker_operations import OperationLedger
        from worker_common import WorkerError
        with tempfile.TemporaryDirectory() as directory:
            calls = []
            def execute(method, params):
                calls.append(params)
                raise SystemExit("owner died after reserving the operation")
            params = {"request_id": "pending", "session_id": "session", "data": "once"}
            reconcile = lambda method, result: result
            with self.assertRaises(SystemExit):
                OperationLedger(Path(directory)).invoke("interact.write", params, execute, reconcile)
            with self.assertRaises(WorkerError) as error:
                OperationLedger(Path(directory)).invoke("interact.write", params, execute, reconcile)
            self.assertEqual(error.exception.kind, "outcome_unknown")
            self.assertEqual(len(calls), 1)


if __name__ == "__main__":
    unittest.main()
