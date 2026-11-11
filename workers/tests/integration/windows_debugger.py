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
import subprocess
import tempfile
import time

from worker_common import safe_env, sha256_file
from worker_runtime import Runtime

FIXTURE = r'''
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <dbghelp.h>
#include <wchar.h>
volatile LONG fixture_value = 0x12345678;
__declspec(noinline) LONG fixture_step(LONG value) { return value ^ 0x1020304; }
int wmain(int argc, wchar_t **argv) {
    if (argc == 3 && wcscmp(argv[1], L"--replay") == 0) {
        SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX);
        HANDLE file = CreateFileW(argv[2], GENERIC_READ, FILE_SHARE_READ, NULL, OPEN_EXISTING, 0, NULL);
        if (file == INVALID_HANDLE_VALUE) return 4;
        char input[5] = {0}; DWORD count = 0;
        BOOL read = ReadFile(file, input, sizeof(input), &count, NULL);
        CloseHandle(file);
        if (read && count == 5 && input[0]=='C' && input[1]=='R' && input[2]=='A' && input[3]=='S' && input[4]=='H')
            RaiseException(EXCEPTION_ACCESS_VIOLATION, EXCEPTION_NONCONTINUABLE, 0, NULL);
        return 0;
    }
    if (argc == 3 && wcscmp(argv[1], L"--dump") == 0) {
        HANDLE file = CreateFileW(argv[2], GENERIC_WRITE, 0, NULL, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, NULL);
        if (file == INVALID_HANDLE_VALUE) return 2;
        BOOL result = MiniDumpWriteDump(GetCurrentProcess(), GetCurrentProcessId(), file,
            MiniDumpWithFullMemory, NULL, NULL, NULL);
        CloseHandle(file);
        return result ? 0 : 3;
    }
    for (;;) { fixture_value = fixture_step(fixture_value); Sleep(20); }
}
'''


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


def run_architecture(root, helper, engine_dir, architecture):
    target, pdb = build_fixture(root, architecture)
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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state",required=True)
    parser.add_argument("--helper",required=True)
    parser.add_argument("--engine-dir",required=True)
    args=parser.parse_args()
    if os.name != "nt":
        raise SystemExit("real DbgEng acceptance requires Windows")
    root=Path(args.state).resolve()
    root.mkdir(parents=True,exist_ok=True)
    helper=Path(args.helper).resolve(strict=True)
    engine=Path(args.engine_dir).resolve(strict=True)
    temporary=Path(tempfile.mkdtemp(prefix="dbgeng-",dir=root))
    print(json.dumps({"attempt_directory":str(temporary)}),flush=True)
    results=[run_architecture(temporary,helper,engine,arch) for arch in ["x64","x86"]]
    report={"passed":True,"scope":"trusted_development_fixture_debugger_adapter", "production_isolation_verified":False,
            "offline_installation_verified":False,"architectures":results}
    destination=root/"acceptance-results.json"
    destination.write_text(json.dumps(report,indent=2),encoding="utf-8")
    print(json.dumps({"passed":True,"architectures":["x64","x86"],"results":str(destination)}))


if __name__ == "__main__":
    main()
