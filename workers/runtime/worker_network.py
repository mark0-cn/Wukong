"""Declarative, bounded network verification against the prepared challenge only."""
import base64
import hashlib
import json
from pathlib import Path
import re
import socket
import time
from worker_common import MAX_FILE, atomic_json, confined
from worker_launch_spec import read_launch, digest

LIMIT = 1024 * 1024
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


def blob(value):
    if not isinstance(value, str) or not value or len(value) > 4*LIMIT//3+4:
        raise ValueError("nonempty bounded base64 bytes required")
    result = base64.b64decode(value, validate=True)
    if not result or len(result) > LIMIT: raise ValueError("decoded bytes exceed bounds")
    return result


def uint(value, maximum=LIMIT):
    if not isinstance(value, int) or isinstance(value, bool) or not 0 <= value <= maximum:
        raise ValueError("integer outside protocol bounds")
    return value


def validate_plan(plan, input_bytes):
    if not isinstance(plan, dict) or set(plan) != {"steps"}: raise ValueError("network plan needs only steps")
    steps = plan["steps"]
    if not isinstance(steps, list) or not 2 <= len(steps) <= 64 or len(input_bytes) > LIMIT:
        raise ValueError("network replay supports 2..64 steps and input <=1 MiB")
    captures, received, sent = set(), False, False
    for step in steps:
        op = step.get("op")
        allowed = {"send":{"op","offset","length","patches"}, "receive":{"op","until_base64","max_bytes"},
                   "capture":{"op","name","offset","length","encoding"}, "expect":{"op","contains_base64"}}
        if op not in allowed or set(step)-allowed[op]: raise ValueError("unsupported network step or field")
        if op == "send":
            offset, length = uint(step["offset"]), uint(step["length"])
            if not length or offset+length > len(input_bytes): raise ValueError("send slice exceeds input")
            patches = step.get("patches", [])
            if not isinstance(patches,list) or len(patches) > 32: raise ValueError("too many or invalid send patches")
            for patch in patches:
                if set(patch)-{"offset","capture","addend","width","endianness"}: raise ValueError("invalid patch field")
                if patch["capture"] not in captures or uint(patch["width"],8) not in {1,2,4,8} or patch["endianness"] not in {"little","big"}:
                    raise ValueError("invalid capture patch")
                if uint(patch["offset"])+patch["width"] > length: raise ValueError("patch exceeds send slice")
                addend=patch.get("addend",0)
                if not isinstance(addend,int) or isinstance(addend,bool) or not -(2**63) <= addend < 2**63: raise ValueError("patch addend must be int64")
            sent, received = True, False
        elif op == "receive":
            if not 1 <= uint(step["max_bytes"]) <= 65536 or len(blob(step["until_base64"])) > step["max_bytes"]:
                raise ValueError("receive delimiter exceeds step budget")
            received = True
        elif op == "capture":
            if not received or not re.fullmatch(r"[A-Za-z][A-Za-z0-9_]{0,31}",step["name"]) or step["name"] in captures:
                raise ValueError("capture needs a unique name and preceding receive")
            if not 1 <= uint(step["length"],32) or step["encoding"] not in {"hex","little","big"}: raise ValueError("invalid capture encoding/length")
            uint(step["offset"])
            if step["encoding"] != "hex" and step["length"] > 8: raise ValueError("integer capture exceeds uint64")
            captures.add(step["name"])
        else:
            if not sent or not received: raise ValueError("expect requires a response after sending input")
            blob(step["contains_base64"])
    if steps[-1]["op"] != "expect": raise ValueError("final network step must assert received bytes")


def replay_network(root, directory, spec, input_bytes, index):
    binding = network_profile(root, spec, spec.get("trusted_local",False))
    if binding != spec["network_binding"]: raise ValueError("prepared service identity changed after admission")
    plan = spec["network"]
    validate_plan(plan, input_bytes)
    trial = directory/("trial-%04d" % index)
    trial.mkdir()
    transcript, wire, captures, received, pending = [], [], {}, None, bytearray()
    deadline = time.monotonic()+spec["timeout_secs"]
    count = 0
    pending_stale = 0
    fresh_received = None
    result = {"trial":index,"input_sha256":sha(input_bytes),"network_binding":binding,
              "state":"completed","network_success":False,"fresh_connection":False,"server_reset":False}

    def check():
        if (directory/"cancel").exists(): raise InterruptedError("network replay cancelled")
        if time.monotonic() >= deadline: raise TimeoutError("network trial deadline exceeded")

    try:
        check()
        with socket.create_connection((binding["transport_host"],binding["transport_port"]),timeout=min(2,spec["timeout_secs"])) as connection:
            connection.settimeout(.2)
            result["fresh_connection"] = True
            result["peer"] = list(connection.getpeername())
            for index, step in enumerate(plan["steps"]):
                check()
                op = step["op"]
                if op == "send":
                    data=bytearray(input_bytes[step["offset"]:step["offset"]+step["length"]])
                    for patch in step.get("patches",[]):
                        value=captures[patch["capture"]]+patch.get("addend",0)
                        width=patch["width"]
                        if not 0 <= value < 2**(8*width): raise ValueError("patched address does not fit; no implicit wraparound")
                        data[patch["offset"]:patch["offset"]+width]=value.to_bytes(width,patch["endianness"])
                    event={"step":index,"direction":"send"}; transcript.append(event)
                    written=0
                    try:
                        while written<len(data):
                            check()
                            if count+len(data)-written>LIMIT or len(wire)>=4096: raise ValueError("transcript budget exceeded")
                            try: amount=connection.send(data[written:])
                            except socket.timeout: continue
                            if not amount: raise EOFError("connection closed during send")
                            wire.append({"step":index,"direction":"send","data_base64":base64.b64encode(data[written:written+amount]).decode()})
                            written+=amount;count+=amount
                    finally: event["data_base64"]=base64.b64encode(data[:written]).decode()
                    received=None
                    fresh_received=None
                    pending_stale=len(pending)
                elif op == "receive":
                    data=bytearray(); fresh=bytearray(); delimiter=blob(step["until_base64"])
                    event={"step":index,"direction":"receive"};transcript.append(event)
                    def consume(amount):
                        nonlocal pending_stale
                        stale=min(amount,pending_stale)
                        data.extend(pending[:amount]);fresh.extend(pending[stale:amount])
                        del pending[:amount];pending_stale-=stale
                    try:
                        while True:
                            check()
                            found=pending.find(delimiter)
                            if found>=0:
                                take=found+len(delimiter)
                                if len(data)+take>step["max_bytes"]: raise ValueError("receive step byte budget exceeded")
                                consume(take)
                                break
                            if pending:
                                # Keep delimiter overlap until the next read.
                                take=max(0,len(pending)-len(delimiter)+1)
                                consume(take)
                            if len(data)+len(pending)>=step["max_bytes"] or count>=LIMIT or len(wire)>=4096: raise ValueError("receive/transcript budget exceeded")
                            try: chunk=connection.recv(min(4096,step["max_bytes"]-len(data)-len(pending),LIMIT-count))
                            except socket.timeout: continue
                            if not chunk: raise EOFError("EOF before delimiter")
                            wire.append({"step":index,"direction":"receive","data_base64":base64.b64encode(chunk).decode()})
                            pending.extend(chunk);count+=len(chunk)
                        received=bytes(data)
                        fresh_received=bytes(fresh)
                    finally: event["data_base64"]=base64.b64encode(data).decode()
                elif op == "capture":
                    data=received[step["offset"]:step["offset"]+step["length"]]
                    if len(data)!=step["length"]: raise ValueError("capture exceeds response")
                    if step["encoding"]=="hex":
                        text=data.decode("ascii")
                        if not re.fullmatch(r"(?:0x)?[0-9a-fA-F]+",text): raise ValueError("capture is not hexadecimal")
                        value=int(text,16)
                    else: value=int.from_bytes(data,step["encoding"])
                    if value>=2**64: raise ValueError("capture exceeds uint64")
                    captures[step["name"]]=value
                elif blob(step["contains_base64"]) not in fresh_received: raise ValueError("fresh response bytes do not match expectation")
            result["network_success"]=True
    except (OSError,ValueError,EOFError) as error:
        result.update(state="cancelled" if isinstance(error,InterruptedError) else "timed_out" if isinstance(error,TimeoutError) else "failed",error=str(error)[:512])
    finally:
        # Pending bytes were actually received, even when a delimiter was absent.
        if pending: transcript.append({"direction":"receive_pending","data_base64":base64.b64encode(pending).decode()})
        raw={"binding":binding,"events":wire,"steps":transcript,"captures":captures,"result":result}
        path=trial/"network-transcript.json";atomic_json(path,raw)
        result["transcript_sha256"]=sha(path.read_bytes())
        result["transcript_path"]=path.relative_to(root).as_posix()
    return result


class NetworkService:
    methods=frozenset(["network.profile"])
    def __init__(self,root,trusted,isolated): self.root,self.trusted=root,trusted
    def dispatch(self,method,params): return network_profile(self.root,params,self.trusted)
