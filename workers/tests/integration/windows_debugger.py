"""Real DbgEng acceptance using self-built trusted x64/x86 fixtures, never installers.

Run on a Windows development host or isolated runner with MSVC and Debugging Tools.
This proves the debugger adapter, not Windows VM isolation or offline installation.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile
import time
import uuid

from worker_common import safe_env, sha256_file
from worker_runtime import Runtime
from support.windows_identity import GUID, pe, dump, pdb

FIXTURE = (Path(__file__).resolve().parents[2] / "windows/fixtures/probe.c").read_text(encoding="utf-8")


def compiler_environment(architecture):
    root = os.environ.get("VSINSTALLDIR")
    if not root:
        locator = Path(os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)")) / "Microsoft Visual Studio/Installer/vswhere.exe"
        root = subprocess.check_output([str(locator), "-latest", "-products", "*", "-requires",
            "Microsoft.VisualStudio.Component.VC.Tools.x86.x64", "-property", "installationPath"], text=True).strip()
    if not root:
        raise RuntimeError("MSVC installation is unavailable")
    setup = Path(root) / "Common7/Tools/VsDevCmd.bat"
    # cmd.exe has different quoting from the CRT argv parser; pass its complete
    # quoted command rather than round-tripping it through list2cmdline.
    command = f'cmd.exe /u /d /s /c ""{setup}" -no_logo -host_arch=x64 -arch={architecture} >nul && set"'
    result = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if result.returncode:
        raise RuntimeError("MSVC environment setup failed: " + result.stderr.decode("utf-16-le",errors="replace")[:2048])
    output = result.stdout.decode("utf-16-le")
    environment = {key.upper():value for key,value in os.environ.items()}
    for line in output.splitlines():
        if "=" in line and not line.startswith("="):
            key, value = line.split("=", 1)
            environment[key.upper()] = value
    return environment


def build_fixture(root, architecture):
    directory = root / ("fixture_" + architecture)
    directory.mkdir()
    source = directory / "fixture.c"
    source.write_text(FIXTURE, encoding="utf-8")
    target = directory / ("fixture_" + architecture + ".exe")
    pdb = target.with_suffix(".pdb")
    environment = compiler_environment(architecture)
    compiler = Path(environment["VCTOOLSINSTALLDIR"]) / "bin/Hostx64" / architecture / "cl.exe"
    process = subprocess.run([str(compiler), "/nologo", "/W4", "/Zi", "/Od", "/MT", "/Fe:" + str(target),
        str(source), "dbghelp.lib", "/link", "/DEBUG", "/PDB:" + str(pdb)],
        cwd=directory, env=environment, capture_output=True, text=True)
    if process.returncode:
        raise RuntimeError("fixture compiler failed: " + process.stdout + process.stderr)
    return target, pdb


def checked(runtime, method, **params):
    return runtime.dispatch(method, params)


def trusted_replay(runtime, image):
    payload = b"CRASH: self-built trusted fixture"
    input_file = checked(runtime, "file.put", path="replay-input.bin",
        data_base64=__import__("base64").b64encode(payload).decode())
    job = checked(runtime, "poc.verify", target=image["path"], input=input_file["path"],
        args=["--replay", "@@"], trials=3, timeout_secs=3,
        call_id="native-fixture-replay", producer_agent_id="fixture-producer", verifier_agent_id="fixture-verifier")
    deadline = time.monotonic() + 20
    while job["state"] not in {"completed", "failed", "cancelled", "lost"}:
        if time.monotonic() >= deadline:
            checked(runtime, "job.cancel", job_id=job["job_id"])
            raise AssertionError("trusted native replay did not complete")
        time.sleep(0.05)
        job = checked(runtime, "job.status", job_id=job["job_id"])
    result = job.get("result", {})
    assert job["state"] == "completed" and result.get("passed") is True, job
    assert result["attempts"] == result["successes"] == 3, result
    assert result["producer_agent_id"] != result["verifier_agent_id"], result
    assert result["target"]["sha256"] == image["sha256"], result
    assert result["input"]["sha256"] == hashlib.sha256(payload).hexdigest(), result
    assert result["fresh_working_directory_per_trial"], result
    directory = runtime.root / "jobs" / job["job_id"]
    trials = sorted(directory.glob("trial-*"))
    assert len(trials) == 3 and len({path.resolve() for path in trials}) == 3, trials
    assert all(sha256_file(path / "input.bin") == hashlib.sha256(payload).hexdigest() for path in trials), trials
    assert len({row["trial"] for row in result["results"]}) == 3, result
    assert all(row["crash_signature"] == "exception:0xc0000005" for row in result["results"]), result
    assert all(row["metadata_protected"] is False for row in result["results"]), result
    return job


def run_architecture(root, helper, engine_dir, architecture, prebuilt=None):
    if prebuilt is None:
        target, pdb = build_fixture(root, architecture)
    else:
        directory = root / ("fixture_" + architecture)
        shutil.copytree(prebuilt / ("fixture_" + architecture), directory)
        target = directory / ("fixture_" + architecture + ".exe")
        pdb = target.with_suffix(".pdb")
    state = root / ("worker_" + architecture)
    state.mkdir()
    configuration = {"helper":str(helper), "helper_sha256":sha256_file(helper),
                     "engine_dir":str(engine_dir), "engine_sha256":sha256_file(engine_dir / "dbgeng.dll")}
    (state / "windows.backend.json").write_text(json.dumps(configuration), encoding="utf-8")
    runtime = Runtime(state, trusted=True)
    doctor = checked(runtime, "doctor")
    assert doctor["capabilities"]["debug_dbgeng"], doctor
    symbols = state / "files/symbols"
    symbols.mkdir(exist_ok=True)
    shutil.copy2(pdb, symbols / pdb.name)
    image = checked(runtime,"file.put",path=target.name,data_base64=__import__("base64").b64encode(target.read_bytes()).decode(),executable=True)
    dump_path = state / "self.dmp"
    made = subprocess.run([str(state / image["path"]), "--dump", str(dump_path)],
                         env=safe_env(), timeout=20, capture_output=True)
    assert made.returncode == 0, (made.returncode, made.stderr)
    checked(runtime,"file.put",path="self.dmp",data_base64=__import__("base64").b64encode(dump_path.read_bytes()).decode())
    evidence = {"architecture":architecture,"target_sha256":image["sha256"],"dump_sha256":sha256_file(dump_path),
                "fixture_source_sha256":hashlib.sha256(FIXTURE.encode()).hexdigest(),"doctor":doctor}
    evidence["trusted_replay"] = trusted_replay(runtime, image)
    opened = checked(runtime,"debug.start",engine="dbgeng",mode="dump",target=image["path"],dump="files/self.dmp")
    session = opened["session_id"]
    try:
        event = checked(runtime,"debug.events",session_id=session)["events"][0]["payload"]
        assert event["debuggee_class"] == 2 and event["debuggee_qualifier"] in {1024,1025},event
        assert event["image_correspondence"]["codeview_matched"] is True,event
        assert event["image_correspondence"]["byte_identity_verified"] is False,event
        assert event["symbol_identity"]["pdb_identity_matched"] is True,event
        evidence["dump_debuggee_type"] = event
        registers = checked(runtime,"debug.inspect",session_id=session,kind="registers",limit=64)
        expected = "eip" if architecture == "x86" else "rip"
        pc = next(row["value"] for row in registers["data"]["registers"] if row["name"] == expected)
        assert isinstance(pc,int) and pc > 0, registers
        stack = checked(runtime,"debug.inspect",session_id=session,kind="stack",limit=16)
        assert stack["data"]["frames"], stack
        threads = checked(runtime,"debug.inspect",session_id=session,kind="threads",limit=16)
        assert threads["data"]["threads"], threads
        modules = checked(runtime,"debug.inspect",session_id=session,kind="modules",limit=64)
        module = next(row for row in modules["data"]["modules"] if row["name"].lower() == target.stem.lower())
        memory = checked(runtime,"debug.inspect",session_id=session,kind="memory",address=module["base"],length=2)
        assert __import__("base64").b64decode(memory["data"]["data_base64"]) == b"MZ", memory
        instructions = checked(runtime,"debug.inspect",session_id=session,kind="disassembly",address=pc,length=32,limit=4)
        assert instructions["data"]["instructions"], instructions
        exception = checked(runtime,"debug.inspect",session_id=session,kind="exception")
        try:
            checked(runtime,"debug.control",session_id=session,action="continue",request_id="dump-control")
            raise AssertionError("dump accepted execution control")
        except ValueError as error:
            assert "dump" in str(error), error
        evidence["dump"] = {"opened":opened,"registers":registers,"stack":stack,"threads":threads,
            "modules":modules,"memory":memory,"instructions":instructions,"exception":exception}
    finally:
        checked(runtime,"debug.close",session_id=session)
    live = checked(runtime,"debug.start",engine="dbgeng",target=image["path"])
    session = live["session_id"]
    try:
        event = checked(runtime,"debug.events",session_id=session)["events"][0]["payload"]
        assert event["debuggee_class"] == 2 and event["debuggee_qualifier"] == 0,event
        evidence["live_debuggee_type"] = event
        added = checked(runtime,"debug.control",session_id=session,action="breakpoint_add",
            symbol=target.stem+"!fixture_step",request_id="breakpoint-add")
        assert checked(runtime,"debug.control",session_id=session,action="breakpoint_add",
            symbol=target.stem+"!fixture_step",request_id="breakpoint-add") == added
        identifier = added["breakpoint_id"]
        resumed = checked(runtime,"debug.control",session_id=session,action="continue",timeout_secs=3,request_id="resume")
        assert resumed["state"]["execution_state"] == "stopped", resumed
        assert resumed["state"]["interruption"] is None, resumed
        stack = checked(runtime,"debug.inspect",session_id=session,kind="stack",limit=16)
        assert any("fixture_step" in (row.get("symbol") or {}).get("name", "") for row in stack["data"]["frames"]),stack
        stepped = checked(runtime,"debug.control",session_id=session,action="step",timeout_secs=3,request_id="step")
        assert stepped["state"]["execution_state"] == "stopped", stepped
        finished = checked(runtime,"debug.control",session_id=session,action="finish",timeout_secs=3,request_id="finish")
        assert finished["state"]["execution_state"] == "stopped", finished
        assert finished["state"]["interruption"] is None, finished
        checked(runtime,"debug.control",session_id=session,action="breakpoint_disable",breakpoint_id=identifier,request_id="disable")
        bounded = checked(runtime,"debug.control",session_id=session,action="continue",timeout_secs=1,request_id="bounded-continue")
        assert bounded["state"]["interruption"] == "deadline", bounded
        assert bounded["state"]["execution_state"] == "stopped", bounded
        assert checked(runtime,"debug.inspect",session_id=session,kind="stack",limit=8)["data"]["frames"]
        checked(runtime,"debug.control",session_id=session,action="breakpoint_enable",breakpoint_id=identifier,request_id="enable")
        checked(runtime,"debug.control",session_id=session,action="breakpoint_remove",breakpoint_id=identifier,request_id="remove")
        evidence["live"] = {"opened":live,"breakpoint":added,"resumed":resumed,"stack":stack,"stepped":stepped,"finished":finished,"bounded":bounded}
    finally:
        checked(runtime,"debug.close",session_id=session)
    return evidence


def reject_mismatched_identity(root, architecture, other, symbol_identity):
    state = root / ("worker_" + architecture)
    runtime = Runtime(state, trusted=True)
    target = state / "files" / ("fixture_" + architecture + ".exe")
    source_pdb = root / ("fixture_" + architecture) / ("fixture_" + architecture + ".pdb")
    cached = state / "files/symbols" / source_pdb.name
    params = dict(engine="dbgeng", mode="dump", target="files/" + target.name, dump="files/self.dmp")
    # Mutate only the exact known RSDS marker in our own SDK-generated fixture,
    # preserving dump structure. DbgEng must observe this dump GUID even while
    # the configured cache contains the original image's matching PDB.
    original_dump = (state / "files/self.dmp").read_bytes()
    marker = b"RSDS" + uuid.UUID(hex=symbol_identity["guid"]).bytes_le + struct.pack("<I",symbol_identity["age"])
    assert marker in original_dump
    changed_dump = original_dump.replace(marker, b"RSDS" + uuid.UUID(int=1).bytes_le + marker[20:])
    wrong_dump = checked(runtime,"file.put",path="wrong-dump-guid.dmp",
        data_base64=__import__("base64").b64encode(changed_dump).decode())
    try:
        checked(runtime,"debug.start", **{**params, "dump":wrong_dump["path"]})
        raise AssertionError("SDK module identity was inferred from the cache instead of the dump")
    except ValueError as error:
        assert "correspond" in str(error),error
    assert (state / "files/self.dmp").read_bytes() == original_dump
    assert not runtime.active_resources()
    changed = bytearray(target.read_bytes())
    pe = struct.unpack_from("<I", changed, 60)[0]
    timestamp = struct.unpack_from("<I", changed, pe + 8)[0]
    struct.pack_into("<I", changed, pe + 8, timestamp ^ 1)
    wrong = checked(runtime, "file.put", path="wrong-timestamp.exe",
        data_base64=__import__("base64").b64encode(changed).decode(), executable=True)
    try:
        checked(runtime, "debug.start", **{**params, "target":wrong["path"]})
        raise AssertionError("dump accepted a mismatched PE timestamp")
    except ValueError as error:
        assert "correspond" in str(error), error
    assert not runtime.active_resources()
    try:
        other_pdb = root / ("fixture_" + other) / ("fixture_" + other + ".pdb")
        shutil.copyfile(other_pdb, cached)
        try:
            checked(runtime, "debug.start", **params)
            raise AssertionError("debugger accepted a valid PDB for another PE")
        except ValueError as error:
            assert "GUID/age" in str(error), error
        assert not runtime.active_resources()
    finally:
        shutil.copyfile(source_pdb, cached)
    # The index SDK accepts images as well as PDBs. A matching PE renamed to
    # .pdb must not qualify as the local symbol file merely by carrying RSDS.
    try:
        shutil.copyfile(target, cached)
        try:
            checked(runtime, "debug.start", **params)
            raise AssertionError("debugger accepted an image renamed to .pdb")
        except ValueError as error:
            assert "PDB" in str(error), error
        assert not runtime.active_resources()
    finally:
        shutil.copyfile(source_pdb, cached)
    # The matching producer PDB still exists at the path embedded in the image.
    # A fresh helper must not load it when the explicitly configured cache lacks it.
    held = cached.with_name("held-" + cached.name)
    assert not held.exists()
    cached.rename(held)
    try:
        opened = checked(runtime, "debug.start", **{**params, "mode":"launch", "dump":None})
        session = opened["session_id"]
        try:
            event = checked(runtime, "debug.events", session_id=session)["events"][0]["payload"]
            assert event["symbol_identity"]["status"] == "missing", event
            try:
                checked(runtime, "debug.control", session_id=session, action="breakpoint_add",
                        symbol=target.stem+"!fixture_step", request_id="outside-symbol-refusal")
                raise AssertionError("helper loaded symbols from outside the configured cache")
            except ValueError:
                pass
            assert checked(runtime, "debug.inspect", session_id=session, kind="registers")["data"]["registers"]
        finally:
            checked(runtime, "debug.close", session_id=session)
    finally:
        held.rename(cached)
    assert not runtime.active_resources()
    return {"architecture":architecture,"wrong_pe_rejected":True,"wrong_pdb_rejected":True,"wrong_dump_guid_rejected":True,"renamed_pe_as_pdb_rejected":True,
            "missing_symbols_reported":True,"embedded_outside_pdb_ignored":True,"active_resources":0}


def native_format_boundaries(root, helper, engine):
    """Run the actual native parser/SDK, preserving inert hostile format fixtures."""
    directory = root / "identity-formats"
    directory.mkdir()
    symbols = directory / "symbols"
    symbols.mkdir()
    def request(kind, data, reject=False):
        path = directory / ("inert." + {"pe":"exe", "pdb":"pdb", "dump":"dmp"}[kind])
        path.write_bytes(data)
        operation = ({"operation":"identity", "kind":kind, "path":str(path)} if kind != "dump" else
                     {"operation":"open", "mode":"dump", "dump":str(path), "architecture":"x64", "timeout_secs":2})
        process = subprocess.run([str(helper),str(engine),str(symbols)],
            input=(json.dumps({"id":1, **operation})+'\n'+json.dumps({"id":2,"operation":"close"})+'\n').encode(),
            capture_output=True, env=safe_env(), timeout=12)
        responses = [json.loads(line) for line in process.stdout.splitlines()]
        assert responses and responses[0]["id"] == 1, (process.returncode, process.stderr[-2048:])
        response = responses[0]
        if reject:
            assert "error" in response, response
        else:
            assert process.returncode == 0 and "result" in response, response
            assert response["result"]["file_sha256"] == hashlib.sha256(data).hexdigest(), response
        return response
    positives = []
    for machine, architecture in [(0x14c,"x86"),(0x8664,"x64")]:
        observed = request("pe", pe(machine))["result"]
        assert observed["architecture"] == architecture, observed
        assert observed["codeview"] == {"format":"RSDS","guid":GUID.hex,"age":7,"pdb_name":"probe.pdb"}, observed
        positives.append(observed)
    no_cv = pe(record=b"NB10inert unsupported identity")
    assert request("pe",no_cv)["result"]["codeview"] is None
    for mutation in ["offset","optional","debug","cv","duplicate"]:
        data = pe()
        if mutation == "offset": struct.pack_into("<I",data,60,0xffffffff)
        elif mutation == "optional": struct.pack_into("<H",data,148,65535)
        elif mutation == "debug": struct.pack_into("<II",data,152+112+48,0x90000000,28)
        elif mutation == "cv": struct.pack_into("<I",data,512+16,0xffffffff)
        else:
            struct.pack_into("<I",data,152+112+52,56)
            data[540:568] = data[512:540]
        request("pe",data,reject=True)
    # Real compiler PDBs are exercised by both live and dump sessions. These
    # invalid MSF fixtures exercise the SDK's own refusal, not a Python parser.
    for mutation in ["block","map","count","version"]:
        data = pdb()
        if mutation == "block": struct.pack_into("<I",data,32,0xffffffff)
        elif mutation == "map": struct.pack_into("<I",data,512*3,99)
        elif mutation == "count": struct.pack_into("<I",data,512*5,0xffffffff)
        else: struct.pack_into("<I",data,512*4,0)
        request("pdb",data,reject=True)
    for mutation in ["count","length","name","duplicate"]:
        data = dump([{}])
        if mutation == "count": struct.pack_into("<I",data,48,0xffffffff)
        elif mutation == "length": struct.pack_into("<I",data,36,0xffffffff)
        elif mutation == "name":
            location = struct.unpack_from("<I",data,52+20)[0]
            struct.pack_into("<I",data,location,1)
        else:
            struct.pack_into("<I",data,8,2)
            data[44:56] = data[32:44]
        request("dump",data,reject=True)
    path = directory / "protocol.exe"
    path.write_bytes(pe())
    sequence = [{"operation":"identity","kind":"pe","path":str(path)}, {"operation":"doctor"},
                {"operation":"identity","kind":"pdb","path":str(path)}, {"operation":"close"}]
    process = subprocess.run([str(helper),str(engine),str(symbols)],
        input=b"".join((json.dumps({"id":i+1, **row})+'\n').encode() for i,row in enumerate(sequence)),
        capture_output=True, env=safe_env(), timeout=12)
    replies = [json.loads(line) for line in process.stdout.splitlines()]
    assert len(replies) == 4 and replies[0]["result"]["file_sha256"] == sha256_file(path), replies
    assert replies[1]["result"]["engine"] == "dbgeng", replies
    assert "before DbgEng" in replies[2].get("error", "") and not replies[2]["session_invalidated"], replies
    assert replies[3]["result"]["state"] == "closed", replies
    # A partially attempted engine initialization is terminal even if no Engine
    # object was returned; later requests must not re-enter DbgHelp.
    process = subprocess.run([str(helper),str(symbols),str(symbols)],
        input=b'{"id":1,"operation":"doctor"}\n'+(json.dumps({"id":2, **sequence[0]})+'\n').encode(),
        capture_output=True, env=safe_env(), timeout=12)
    replies = [json.loads(line) for line in process.stdout.splitlines()]
    assert len(replies) == 1 and replies[0].get("error") and replies[0]["session_invalidated"], replies
    return {"pe_positive":positives,"pe_negative":5,"pdb_negative":4,"dump_negative":4,
            "identity_after_engine_rejected":True,"initialization_failure_terminal":True,
            "format_readers":["object_pe","pinned_dbghelp_file_index","dbgeng_module_sdk"]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state",required=True)
    parser.add_argument("--helper",required=True)
    parser.add_argument("--engine-dir",required=True)
    parser.add_argument("--prebuilt-fixtures", help="Explicit self-built trusted x64/x86 fixture directory for a compiler-free test VM")
    args=parser.parse_args()
    if os.name != "nt":
        raise SystemExit("real DbgEng acceptance requires Windows")
    root=Path(args.state).resolve()
    root.mkdir(parents=True,exist_ok=True)
    helper=Path(args.helper).resolve(strict=True)
    engine=Path(args.engine_dir).resolve(strict=True)
    temporary=Path(tempfile.mkdtemp(prefix="dbgeng-",dir=root))
    print(json.dumps({"attempt_directory":str(temporary)}),flush=True)
    prebuilt = Path(args.prebuilt_fixtures).resolve(strict=True) if args.prebuilt_fixtures else None
    formats = native_format_boundaries(temporary, helper, engine)
    results=[run_architecture(temporary,helper,engine,arch,prebuilt) for arch in ["x64","x86"]]
    identity_checks = [reject_mismatched_identity(temporary, row["architecture"], other, row["dump_debuggee_type"]["symbol_identity"])
                       for row, other in zip(results,["x86","x64"])]
    report={"passed":True,"scope":"trusted_development_fixture_debugger_adapter", "production_isolation_verified":False,
            "offline_installation_verified":False,"architectures":results,"image_correspondence":identity_checks,
            "native_format_boundaries":formats}
    destination=root/"acceptance-results.json"
    destination.write_text(json.dumps(report,indent=2),encoding="utf-8")
    print(json.dumps({"passed":True,"architectures":["x64","x86"],"results":str(destination)}))


if __name__ == "__main__":
    main()
