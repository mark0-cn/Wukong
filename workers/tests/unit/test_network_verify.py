"""Real loopback protocol trials, with no arbitrary script execution by the verifier."""
import base64
import hashlib
import json
from pathlib import Path
import secrets
import socketserver
import tempfile
import threading
import time
import unittest
from unittest.mock import patch
from worker_network import network_profile, replay_network, validate_plan
from worker_runtime import Runtime


def b64(data): return base64.b64encode(data).decode()


class Server(socketserver.ThreadingTCPServer):
    allow_reuse_address=True
    daemon_threads=True


class Protocol(socketserver.StreamRequestHandler):
    def handle(self):
        self.request.settimeout(2)
        nonce=secrets.randbits(48)
        self.server.nonces.append(nonce)
        if self.server.mode=="silent":
            self.server.release.wait(2)
            return
        banner=b"nonce:%016x\n> " % nonce
        if self.server.mode=="banner": banner+=b"accepted:marker\n"
        self.wfile.write(banner);self.wfile.flush()
        if self.rfile.readline(64)!=b"HELLO\n": return
        self.wfile.write(b"proof> ");self.wfile.flush()
        try: data=self.rfile.read(8)
        except (ConnectionError,TimeoutError): return
        reply=b"accepted:marker\n" if int.from_bytes(data,"little")==nonce+1 else b"rejected\n"
        self.wfile.write(reply);self.wfile.flush()


class NetworkVerificationTests(unittest.TestCase):
    def setUp(self):
        self.temporary=tempfile.TemporaryDirectory();self.addCleanup(self.temporary.cleanup)
        self.root=Path(self.temporary.name)
        self.base=self.root/"files/challenge";self.base.mkdir(parents=True)
        self.target=self.base/"target";self.target.write_bytes(b"trusted protocol fixture identity; never executed")
        self.server=Server(("127.0.0.1",0),Protocol)
        self.server.mode="normal";self.server.nonces=[];self.server.release=threading.Event()
        self.thread=threading.Thread(target=self.server.serve_forever,kwargs={"poll_interval":.02},daemon=True);self.thread.start()
        self.addCleanup(self.close_server)
        self.launch={"schema_version":1,"challenge_id":"network-fixture","base":"files/challenge","target":"target",
                     "files":{"target":hashlib.sha256(self.target.read_bytes()).hexdigest()},
                     "service":{"mode":"local_service","port":self.server.server_address[1],"host":None}}
        self.save_launch()
        self.input=b"HELLO\n"+b"\0"*8
        self.plan={"steps":[{"op":"receive","until_base64":b64(b"> "),"max_bytes":64},
                            {"op":"capture","name":"nonce","offset":6,"length":16,"encoding":"hex"},
                            {"op":"send","offset":0,"length":6},
                            {"op":"receive","until_base64":b64(b"proof> "),"max_bytes":64},
                            {"op":"send","offset":6,"length":8,"patches":[{"offset":0,"capture":"nonce","addend":1,"width":8,"endianness":"little"}]},
                            {"op":"receive","until_base64":b64(b"\n"),"max_bytes":64},
                            {"op":"expect","contains_base64":b64(b"accepted:marker")}]}
        self.spec={"target":"files/challenge/target","launch_spec":"files/challenge/launch.json",
                   "network":self.plan,"trusted_local":True,"timeout_secs":2}
        self.spec["network_binding"]=network_profile(self.root,self.spec,True)
        self.trials=self.root/"trials";self.trials.mkdir()

    def close_server(self):
        self.server.release.set();self.server.shutdown();self.server.server_close();self.thread.join(2)

    def save_launch(self): (self.base/"launch.json").write_text(json.dumps(self.launch))

    def replay(self,index=0): return replay_network(self.root,self.trials,self.spec,self.input,index)

    def test_live_multiround_capture_patch_and_fresh_connections(self):
        validate_plan(self.plan,self.input)
        for index in range(2):
            result=self.replay(index)
            self.assertTrue(result["network_success"],result)
            raw=(self.root/result["transcript_path"]).read_bytes()
            self.assertEqual(hashlib.sha256(raw).hexdigest(),result["transcript_sha256"])
            transcript=json.loads(raw)
            self.assertEqual(transcript["captures"]["nonce"],self.server.nonces[index])
            sent=b"".join(base64.b64decode(e["data_base64"]) for e in transcript["events"] if e["direction"]=="send")
            self.assertEqual(sent,b"HELLO\n"+(self.server.nonces[index]+1).to_bytes(8,"little"))
            self.assertFalse(result["server_reset"])
            self.assertFalse(result["network_binding"]["runtime_identity_confirmed"])
        self.assertEqual(len(self.server.nonces),2)
        self.assertNotEqual(*self.server.nonces)

    def test_negative_response_and_eof_are_not_success(self):
        self.plan["steps"][4]["patches"][0]["addend"]=2
        result=self.replay()
        self.assertFalse(result["network_success"])
        self.assertEqual(result["state"],"failed")
        self.input=b"WRONG\n"+b"\0"*8
        result=self.replay(1)
        self.assertFalse(result["network_success"])
        self.assertIn("EOF",result["error"])

    def test_timeout_and_cancellation_preserve_negative_evidence(self):
        self.server.mode="silent";self.spec["timeout_secs"]=.1
        result=self.replay()
        self.assertEqual(result["state"],"timed_out")
        (self.trials/"cancel").touch()
        result=self.replay(1)
        self.assertEqual(result["state"],"cancelled")
        self.assertFalse(result["network_success"])

    def test_old_banner_cannot_satisfy_success_after_send(self):
        self.plan["steps"]=self.plan["steps"][:3]+[
            {"op":"receive","until_base64":b64(b"\n"),"max_bytes":64},
            {"op":"expect","contains_base64":b64(b"accepted:marker")}]
        # Control recv chunking: TCP may otherwise split a single server write,
        # which cannot establish when unread bytes were generated by the peer.
        with patch("worker_network.socket.create_connection") as connect:
            connection=connect.return_value.__enter__.return_value
            connection.getpeername.return_value=("127.0.0.1",self.launch["service"]["port"])
            connection.recv.return_value=b"nonce:0000000000000001\n> accepted:marker\n"
            connection.send.side_effect=lambda data: len(data)
            result=self.replay()
        self.assertFalse(result["network_success"],result)
        self.assertIn("fresh response",result["error"])

    def test_patch_overflow_and_capture_bounds_preserve_failed_transcripts(self):
        self.plan["steps"][4]["patches"][0].update(width=1,addend=2**63-1)
        result=self.replay()
        self.assertFalse(result["network_success"])
        self.assertIn("does not fit",result["error"])
        self.assertTrue((self.root/result["transcript_path"]).is_file())
        self.plan["steps"][1]["offset"]=999
        result=self.replay(1)
        self.assertFalse(result["network_success"])
        self.assertIn("capture exceeds response",result["error"])

    def test_binding_changed_and_arbitrary_endpoint_rejected(self):
        self.launch["service"]["port"]+=1;self.save_launch()
        with self.assertRaisesRegex(ValueError,"identity changed"): self.replay()
        self.spec["launch_spec"]="files/forged.json"
        with self.assertRaisesRegex(ValueError,"host-prepared"): network_profile(self.root,self.spec,True)
        self.spec["launch_spec"]="files/challenge/launch.json"
        self.target.write_bytes(b"changed")
        with self.assertRaisesRegex(ValueError,"input changed"): network_profile(self.root,self.spec,True)

    def test_plan_bounds_and_illegal_dataflow(self):
        for plan in [{"steps":[]},{"steps":[{"op":"expect","contains_base64":b64(b"x")}]*2},
                     {"steps":[{"op":"send","offset":0,"length":99999},{"op":"expect","contains_base64":b64(b"x")}]},
                     {"steps":[{"op":"receive","until_base64":b64(b"x"),"max_bytes":999999},{"op":"expect","contains_base64":b64(b"x")}]}]:
            with self.assertRaises(ValueError): validate_plan(plan,self.input)

    def test_durable_worker_job_replay_and_identity_accounting(self):
        runtime=Runtime(self.root,True)
        (self.root/"input.bin").write_bytes(self.input)
        params={**self.spec,"input":"input.bin","request_id":"network-request","call_id":"network-call",
                "producer_agent_id":"producer","verifier_agent_id":"verifier","trials":2}
        first=runtime.dispatch("poc.verify",params)
        deadline=time.monotonic()+8
        while time.monotonic()<deadline:
            status=runtime.dispatch("job.status",{"job_id":first["job_id"]})
            if status["state"] in {"completed","failed","lost"}: break
            time.sleep(.03)
        self.assertEqual(status["state"],"completed",status)
        self.assertTrue(status["result"]["passed"],status)
        self.assertEqual(status["result"]["successes"],2)
        self.assertEqual(status["result"]["observed_stage"],"network_behavior")
        self.assertEqual(runtime.dispatch("poc.verify",params)["job_id"],first["job_id"])
        self.assertEqual(len(self.server.nonces),2)
        # Terminal metadata precedes process exit; wait for the owned supervisor
        # to release Windows handles before the temporary state is removed.
        from worker_processes import process_identity
        deadline=time.monotonic()+3
        while time.monotonic()<deadline and process_identity(status["supervisor_pid"]) not in {"missing","unknown"}:
            time.sleep(.02)

if __name__ == "__main__": unittest.main()
