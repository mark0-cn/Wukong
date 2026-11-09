"""Structured crash observations and exact-byte cyclic offsets."""
import base64
import hashlib
import json
from pathlib import Path
import re
from worker_common import MAX_FILE, confined


def integer(value):
    match = re.match(r"^(0x[0-9a-fA-F]+|[0-9]+)(?:\s|$)", str(value))
    return int(match.group(1), 16 if match.group(1).startswith("0x") else 10) if match else None


def normalize_address(address, profile):
    for mapping in profile.get("maps", []):
        if address is not None and mapping["start"] <= address < mapping["end"]:
            module = next((m for m in profile.get("modules", []) if m.get("inode") == mapping["inode"] and m.get("device") == mapping["device"]), {})
            return {"space":"mapped_file_offset", "value":address-mapping["start"]+mapping["offset"],
                    "module_sha256":module.get("sha256"), "path":mapping.get("path")}
    return {"space":"runtime", "value":address, "module_sha256":None}


def location_signature(snapshot):
    pc = snapshot.get("pc_location", {})
    if not pc.get("module_sha256") or not snapshot.get("signal"): return None
    frames = [f["location"] for f in snapshot.get("frames", []) if f.get("location", {}).get("module_sha256")][:6]
    stable = {"signal":snapshot["signal"], "pc":[pc["module_sha256"],pc["value"]],
              "frames":[[f["module_sha256"],f["value"]] for f in frames]}
    return "location:" + hashlib.sha256(json.dumps(stable,sort_keys=True).encode()).hexdigest()


def debugger_snapshot(session):
    with session.lock:
        info=session.command("-thread-info",10,True)
        threads=(info.get("payload") or {}).get("threads",[])
        if not threads or any(t.get("state") != "stopped" for t in threads):
            raise ValueError("snapshot requires stopped threads; interrupt explicitly before observing")
        raw={"threads":info}; errors={}
        def query(key, command):
            reply=session.command(command,10,True); raw[key]=reply
            if reply["class"] == "error": errors[key]=(reply.get("payload") or {}).get("msg","unavailable")
            return reply.get("payload") or {}
        pc=integer(query("pc",'-data-evaluate-expression "$pc"').get("value"))
        sp=integer(query("sp",'-data-evaluate-expression "$sp"').get("value"))
        fault=integer(query("fault",'-data-evaluate-expression "$_siginfo._sifields._sigfault.si_addr"').get("value"))
        registers=query("registers","-data-list-register-values x")
        stack=query("stack","-stack-list-frames 0 15").get("stack",[])
        if sp is not None: query("stack_memory",f"-data-read-memory-bytes {sp:#x} 128")
        if pc is not None: query("instruction",f"-data-disassemble -s {pc:#x} -e {pc+32:#x} -- 0")
        try: profile=session.profile()["process"]
        except (OSError,ValueError) as error: profile={"status":"unknown","error":str(error)}
        stopped=next((e.get("payload") or {} for e in reversed(session.events) if e.get("message")=="stopped"),{})
        frames=[]
        for frame in stack:
            frame=frame.get("frame",frame)
            frames.append({"level":frame.get("level"),"function":frame.get("func"),"address":frame.get("addr"),
                "location":normalize_address(integer(frame.get("addr")),profile)})
        result={"source":"gdb_mi","session_id":session.id,"signal":stopped.get("signal-name"),
            "stop_reason":stopped.get("reason"),"pc":pc,"sp":sp,"fault_address":fault,
            "pc_location":normalize_address(pc,profile),"frames":frames,"registers":registers,
            "profile":profile,"raw":raw,"errors":errors,"transcript":"debug/"+session.id+"/transcript.mi"}
        result["location_signature"]=location_signature(result)
        result["signature_strength"]="location_and_frames" if result["location_signature"] else "unresolved"
        return result


class CrashService:
    methods=frozenset(["core.snapshot","cyclic.offset"])
    def __init__(self,root,trusted,isolated): self.root=Path(root).resolve()
    def dispatch(self,method,params):
        if method == "cyclic.offset":
            path=confined(self.root,params["pattern"],True)
            if path.stat().st_size > 1048576: raise ValueError("pattern exceeds 1 MiB")
            pattern=path.read_bytes(); observed=base64.b64decode(params["observed_base64"],validate=True)
            n=int(params["n"]); alphabet=params["alphabet"].encode("ascii")
            if not 1 <= n <= 16 or len(observed)!=n or not 2 <= len(alphabet)<=128 or len(set(alphabet))!=len(alphabet):
                raise ValueError("supply exactly n observed bytes and a unique ASCII alphabet")
            from pwnlib.util.cyclic import cyclic
            if bytes(cyclic(len(pattern),alphabet=alphabet,n=n)) != pattern:
                raise ValueError("pattern does not match declared alphabet/n")
            positions=[]; start=0
            while len(positions)<2:
                found=pattern.find(observed,start)
                if found<0: break
                positions.append(found); start=found+1
            return {"pattern_sha256":hashlib.sha256(pattern).hexdigest(),"n":n,"alphabet":params["alphabet"],
                "observed_base64":params["observed_base64"],"offset":positions[0] if len(positions)==1 else None,
                "status":"unique" if len(positions)==1 else "ambiguous" if positions else "not_found"}
        path=confined(self.root,params["core"],True)
        if path.stat().st_size > MAX_FILE: raise ValueError("core exceeds 32 MiB; use live debugger snapshot")
        from pwn import Corefile
        core=Corefile(str(path))
        mappings=[{"start":m.start,"end":m.stop,"path":m.name,"flags":m.flags} for m in core.mappings][:512]
        result={"source":"core","core_sha256":hashlib.sha256(path.read_bytes()).hexdigest(),
            "pc":core.pc,"sp":core.sp,"fault_address":core.fault_addr,"signal":core.signal,
            "registers":core.registers,"mappings":mappings,"signature_strength":"unresolved",
            "note":"Core mapping names do not establish original module hashes. No location signature is fabricated."}
        try: result["stack_base64"]=base64.b64encode(core.read(core.sp,128)).decode()
        except Exception as error: result["stack_error"]=str(error)[:256]
        return result


def replay_snapshot(root, spec, input_path, cancel):
    """Fresh debugger observation for one candidate; no caller-provided signatures."""
    import shlex
    import signal
    import time
    import shutil
    from worker_debugger import DebugSession, parse_mi_response
    if not shutil.which("gdb") or parse_mi_response is None:
        raise ValueError("GDB and pygdbmi are required for location signatures")
    stdin = spec.get("input_mode", "argv") == "stdin"
    args = ([str(a) for a in spec.get("args", []) if a != "@@"] if stdin else
            [str(input_path) if a == "@@" else str(a) for a in spec.get("args", ["@@"])])
    params = {"target":spec["target"], "args":args, "env":spec.get("env")}
    if spec.get("launch_spec"):
        params.pop("args",None); params.pop("env",None)
        params["launch_spec"]=spec["launch_spec"]
        if not stdin: params["input"]=input_path.relative_to(root).as_posix()
    if stdin: params["_stdin"] = input_path.relative_to(root).as_posix()
    session = DebugSession(root, params, isolated=spec.get("isolated", False))
    try:
        command = ('-interpreter-exec console ' + json.dumps("run < " + shlex.quote(str(session.stdin_path)))) if stdin else "-exec-run"
        reply = session.command(command, spec["timeout_secs"], True)
        if reply["class"] == "error": raise ValueError(str(reply.get("payload")))
        deadline = time.monotonic() + spec["timeout_secs"]
        while time.monotonic() < deadline and not cancel.exists() and not session.dead:
            stopped = next((e.get("payload") or {} for e in reversed(session.events) if e.get("message") == "stopped"), {})
            if stopped.get("reason") == "signal-received":
                snapshot = debugger_snapshot(session)
                snapshot["signal_number"] = getattr(signal, snapshot.get("signal", ""), None)
                snapshot["delivery"] = "stdin_regular_file" if stdin else "argv_file"
                return snapshot
            if stopped.get("reason", "").startswith("exited"): return {"signature_strength":"unresolved", "reason":"debuggee exited"}
            time.sleep(0.02)
        return {"signature_strength":"unresolved", "reason":"debuggee timeout, cancellation or debugger exit"}
    finally:
        session.close()
