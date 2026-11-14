#!/usr/bin/env python3
"""Live Linux acceptance test. Executes only the embedded crash fixture.

Run inside the isolated worker image:
  python3 /opt/wukong/tests/run.py integration --state /state/acceptance
Add --fuzz to require AFL++ QEMU and prove that it executes and emits stats.
"""
import argparse
import base64
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import time

FIXTURE = r'''
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
// Keep the fault PC in the admitted fixture image. abort() faults in libc,
// whose path alone cannot establish the content identity required by reduction.
__attribute__((noinline)) static void fixture_crash(void) {
    *(volatile unsigned char *)1 = 0x42;
}
int main(int argc, char **argv) {
    if (argc < 2) { volatile unsigned counter = 0; for (;;) counter++; }
    FILE *file = fopen(argv[1], "rb");
    if (!file) return 3;
    unsigned char input[128] = {0};
    size_t n = fread(input, 1, sizeof(input), file);
    fclose(file);
    if (n >= 3 && input[0] == 'B' && input[1] == 'A' && input[2] == 'D') fixture_crash();
    puts("fixture-ok");
    return 0;
}
'''


from support.client import Client


def debugger_metadata_proof(client, state, session_id):
    marker = state / "debugger-metadata-write.txt"
    expression = '$_shell("id -u > '+str(marker)+'")'
    encoded = json.dumps(expression)
    commands = [f'-data-evaluate-expression {encoded}',
                '-data-evaluate-expression '+encoded.replace('$_',r'\044\137'),
                '-data-evaluate-expression '+encoded.replace('$_',r'\x24\x5f'),
                '-data-evaluate-expression '+encoded.replace('$_',r'\u0024\u005f'),
                f'-data-read-memory-bytes {encoded} 8',
                f'-data-write-memory-bytes {encoded} ff',
                f'-break-condition 1 {encoded}', f'-break-insert {encoded}',
                f'-exec-until {encoded}',
                f'-data-disassemble -s {encoded} -e 0x400100 -- 0']
    rejected = []
    for command in commands:
        try:
            client.call('debug.command',session_id=session_id,command=command)
        except AssertionError as failure:
            reply = failure.args[0]
            assert isinstance(reply,dict) and 'GDB/MI' in reply.get('error',{}).get('message',''),reply
            rejected.append({'command':command,'error':reply['error']})
        else:
            raise AssertionError('unsafe debugger expression reached the interpreter: '+command)
        assert not marker.exists(), 'debugger wrote trusted metadata with an untrusted expression'
    return {'host_calls_rejected':len(rejected),'protected_metadata_unchanged':True,'requests':rejected}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--state", required=True)
    parser.add_argument("--fuzz", action="store_true")
    parser.add_argument("--static-fixture", action="store_true", help="Use a static fixture for debugger-adapter acceptance; does not cover dynamic loader identity")
    args = parser.parse_args()
    state = Path(args.state).resolve()
    client = Client(state)
    doctor = client.call("doctor")
    assert doctor["capabilities"]["debug_gdb_mi"], doctor
    source = client.put("fixture.c", FIXTURE.encode())
    # The acceptance harness compiles its own trusted fixture; analysed-target
    # jobs cannot write the worker's private uploaded-file namespace.
    with tempfile.TemporaryDirectory() as build:
        source_path = Path(build) / "fixture.c"
        target_path = Path(build) / "fixture"
        source_path.write_text(FIXTURE)
        subprocess.run(["gcc", "-g", "-O0", *(["-static"] if args.static_fixture else []), "-o", str(target_path), str(source_path)], check=True)
        client.put("fixture", target_path.read_bytes(), executable=True)
    crash = client.put("crash.bin", b"BAD-padding-to-minimize")
    seed = client.put("seeds/seed", b"BAA")
    started = client.call("debug.start", target="files/fixture", args=[str(state / seed["path"])])
    session_id = started["session_id"]
    debugger_policy = debugger_metadata_proof(client,state,session_id)
    breakpoint = client.call("debug.control", session_id=session_id, action="breakpoint_add", symbol="main", request_id="typed-main-breakpoint")
    assert breakpoint["class"] == "done", breakpoint
    assert client.call("debug.control", session_id=session_id, action="breakpoint_add", symbol="main", request_id="typed-main-breakpoint") == breakpoint
    run = client.call("debug.control", session_id=session_id, action="run", timeout_secs=5, request_id="typed-run")
    assert run["class"] == "running", run
    assert run["state"]["execution_state"] == "stopped" and run["state"]["interruption"] is None, run
    for _ in range(100):
        events = client.call("debug.events", session_id=session_id)
        if any(record.get("type") == "notify" and record.get("message") == "stopped" for record in events["events"]):
            break
        time.sleep(0.05)
    else:
        raise AssertionError(events)
    stack = client.call("debug.command", session_id=session_id, command="-stack-list-frames")
    assert stack["class"] == "done" and 'func="main"' in stack["record"], stack
    memory = client.call("debug.command", session_id=session_id, command="-data-read-memory-bytes main 16")
    assert memory["class"] == "done" and "contents=" in memory["record"], memory
    raw_pc = client.call("debug.command",session_id=session_id,command='-data-evaluate-expression "$pc"')
    assert raw_pc['class']=='done',raw_pc
    typed_stack = client.call("debug.inspect",session_id=session_id,kind="stack",limit=8)
    frame = typed_stack["data"]["stack"][0]
    assert frame["func"] == "main", typed_stack
    pc = int(frame["addr"],16)
    typed_memory = client.call("debug.inspect",session_id=session_id,kind="memory",address=pc,length=16)
    assert len(base64.b64decode(typed_memory["data"]["data_base64"])) == 16, typed_memory
    registers = client.call("debug.inspect",session_id=session_id,kind="registers",limit=64)
    assert registers["data"]["registers"], registers
    threads = client.call("debug.inspect",session_id=session_id,kind="threads",limit=8)
    assert threads["data"]["threads"], threads
    modules = client.call("debug.inspect",session_id=session_id,kind="modules",limit=64)
    assert modules["data"]["process"]["pid"], modules
    disassembly = client.call("debug.inspect",session_id=session_id,kind="disassembly",address=pc,length=32,limit=4)
    assert 1 <= len(disassembly["data"]["asm_insns"]) <= 4, disassembly
    # Relay restart must preserve debugger ownership as well as running jobs.
    client.close()
    client = Client(state)
    assert client.call("debug.command", session_id=session_id, command="-stack-list-frames")["class"] == "done"
    client.call("debug.close", session_id=session_id)
    bounded_session = client.call("debug.start",target="files/fixture")["session_id"]
    try:
        bounded = client.call("debug.control",session_id=bounded_session,action="run",timeout_secs=1,request_id="typed-deadline")
        assert bounded["state"] == {"execution_state":"stopped","interruption":"deadline"}, bounded
        assert client.call("debug.inspect",session_id=bounded_session,kind="stack",limit=8)["data"]["stack"]
    finally:
        client.call("debug.close",session_id=bounded_session)
    replay = client.finish(client.call("crash.replay", target="files/fixture", input=crash["path"], timeout_secs=3))
    assert replay["result"]["passed"] and replay["result"]["observed_stage"] == "crash", replay
    verified = client.finish(client.call("poc.verify", target="files/fixture", input=crash["path"], trials=3, timeout_secs=3,
                                         producer_agent_id="fixture-producer", verifier_agent_id="independent-test-verifier", call_id="fixture-verification"))
    assert verified["result"]["passed"] and verified["result"]["successes"] == 3, verified
    minimized = client.finish(client.call("input.minimize", target="files/fixture", input=crash["path"], timeout_secs=3, max_attempts=100))
    assert minimized["result"]["minimized_size"] == 3, minimized
    data = client.call("file.get", path=minimized["result"]["artifact"]["path"])
    assert base64.b64decode(data["data_base64"]) == b"BAD", data
    results = {"fixture_linkage":"static" if args.static_fixture else "dynamic", "doctor": doctor, "gdb": {"breakpoint": breakpoint, "stack": stack, "memory": memory,
               "typed_stack":typed_stack,"typed_memory":typed_memory,"registers":registers,
               "threads":threads,"modules":modules,"disassembly":disassembly,"bounded":bounded}, "replay": replay,
               "verification": verified, "minimization": minimized,"debugger_policy":debugger_policy,"raw_pc":raw_pc}
    if args.fuzz:
        # Docker Desktop can pipe core notifications to an external handler.
        # This fixture check proves QEMU execution/statistics, not complete crash
        # collection under that host setting. Never mutate host-global sysctls.
        fuzz = client.call("fuzz.start", target="files/fixture", seeds="files/seeds", timeout_secs=8, exec_timeout_ms=1000, memory_mb=256,
                           env={"AFL_I_DONT_CARE_ABOUT_MISSING_CRASHES": "1"})
        fuzz = client.finish(fuzz, timeout=40)
        assert fuzz["state"] == "completed" and fuzz["fuzzer_stats"], fuzz
        assert any(int(stats.get("execs_done", "0")) > 0 for stats in fuzz["fuzzer_stats"].values()), fuzz
        results["fuzz"] = fuzz
    output = state / "acceptance-results.json"
    output.write_text(json.dumps(results, indent=2))
    print(json.dumps({"passed": True, "results": str(output), "fuzz_tested": args.fuzz, "verified_trials": 3, "minimized_bytes": 3}))
    client.close()


if __name__ == "__main__":
    main()
