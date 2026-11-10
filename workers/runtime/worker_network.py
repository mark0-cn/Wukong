"""Declarative, bounded network verification against the prepared challenge only."""
import hashlib
import socket
import time
from worker_common import MAX_FILE, confined
from worker_common import sha256_file as digest
from worker_launch_spec import read_launch

from worker_network_plan import blob, validate_plan, ReplayState
from worker_network_transport import ReplayConnection
from worker_network_evidence import Transcript
LAUNCH = "files/challenge/launch.json"


def sha(data): return hashlib.sha256(data).hexdigest()


def network_profile(root, params, trusted=False):
    if params.get("launch_spec") != LAUNCH:
        raise ValueError("network verification requires the host-prepared files/challenge/launch.json")
    document = read_launch(root, LAUNCH)
    raw, spec = document.raw, document.spec
    service = spec.get("service") or {}
    port = service.get("port")
    if (spec.get("schema_version") != 1 or spec.get("base") != "files/challenge"
            or service.get("mode") not in {"local_service", "remote"}
            or not isinstance(port, int) or isinstance(port, bool) or not 1 <= port <= 65535):
        raise ValueError("LaunchSpec has no supported configured network service")
    files = document.check_files(max_file=MAX_FILE, max_total=64*1024*1024)
    target = confined(root, params["target"], True)
    if target.stat().st_size > MAX_FILE or digest(target) != files[spec["target"]]:
        raise ValueError("network target differs from LaunchSpec")
    return {"launch_spec_sha256":sha(raw), "challenge_id":spec["challenge_id"],
            "target_sha256":files[spec["target"]], "service":service,
            "transport_host":"127.0.0.1" if trusted else "challenge", "transport_port":port,
            "runtime_identity_confirmed":False,
            "identity_limit":"Configured service binding; network I/O does not confirm the deployed binary, libc or image."}


def replay_network(root, directory, spec, input_bytes, index):
    binding = network_profile(root, spec, spec.get("trusted_local", False))
    if binding != spec["network_binding"]:
        raise ValueError("prepared service identity changed after admission")
    plan = spec["network"]
    validate_plan(plan, input_bytes)
    trial = directory / ("trial-%04d" % index)
    trial.mkdir()
    transcript = Transcript()
    state = ReplayState()
    transport = None
    deadline = time.monotonic() + spec["timeout_secs"]
    result = {"trial": index, "input_sha256": sha(input_bytes), "network_binding": binding,
              "state": "completed", "network_success": False, "fresh_connection": False, "server_reset": False}

    def check():
        if (directory / "cancel").exists():
            raise InterruptedError("network replay cancelled")
        if time.monotonic() >= deadline:
            raise TimeoutError("network trial deadline exceeded")

    try:
        check()
        with socket.create_connection((binding["transport_host"], binding["transport_port"]),
                                      timeout=min(2, spec["timeout_secs"])) as connection:
            connection.settimeout(.2)
            result["fresh_connection"] = True
            result["peer"] = list(connection.getpeername())
            transport = ReplayConnection(connection, transcript, check)
            for step_index, step in enumerate(plan["steps"]):
                check()
                op = step["op"]
                if op == "send":
                    transport.send(step_index, state.payload(step, input_bytes))
                    state.received = state.fresh = None
                elif op == "receive":
                    state.received, state.fresh = transport.receive(step_index, blob(step["until_base64"]), step["max_bytes"])
                elif op == "capture":
                    state.capture(step)
                else:
                    state.expect(step)
            result["network_success"] = True
    except (OSError, ValueError, EOFError) as error:
        result.update(state="cancelled" if isinstance(error, InterruptedError) else
                      "timed_out" if isinstance(error, TimeoutError) else "failed", error=str(error)[:512])
    finally:
        transcript.save(root, trial, binding, state.captures, result, transport.pending if transport else b"")
    return result


class NetworkService:
    methods=frozenset(["network.profile"])
    def __init__(self,root,trusted,isolated): self.root,self.trusted=root,trusted
    def dispatch(self,method,params): return network_profile(self.root,params,self.trusted)
