#!/usr/bin/env python3
"""angr symbolic-execution job payload for the execution worker.

Arguments: a single JSON object on argv[1]:
{
  "target": "/absolute/path/to/target.bin",
  "find": [module_relative_address, ...],
  "avoid": [module_relative_address, ...],
  "stdin_length": 64,
  "max_steps": 50000
}
Prints a single JSON line with the solved stdin (base64) and exploration
statistics, or an explicit error. This script runs inside the isolated job;
it never trusts the target and never touches worker metadata.
"""
import base64
import json
import sys
from pathlib import Path


def fail(message):
    print(json.dumps({"solved": False, "error": str(message)[:2048]}))
    sys.exit(1)


def main():
    if len(sys.argv) != 3:
        fail("expected a JSON argument and a target path argument")
    try:
        params = json.loads(sys.argv[1])
    except ValueError:
        fail("argument is not valid JSON")
    target = sys.argv[2]
    if not Path(target).is_absolute():
        # Trusted (non-isolated) jobs run with the state root as cwd; isolated
        # jobs receive the staged absolute path via argv staging.
        target = str((Path.cwd() / target).resolve())
    find = params.get("find") or []
    avoid = params.get("avoid") or []
    stdin_length = int(params.get("stdin_length", 64))
    max_steps = int(params.get("max_steps", 50000))
    if not isinstance(target, str) or not find:
        fail("target and a nonempty find list are required")
    if not (1 <= stdin_length <= 4096):
        fail("stdin_length out of range")
    try:
        import angr  # noqa: PLC0415 - heavy import inside the job
        import claripy  # noqa: PLC0415
    except ImportError:
        fail("angr is not installed in this worker")
    try:
        project = angr.Project(target, auto_load_libs=False)
    except Exception as error:  # noqa: BLE001 - report load failures cleanly
        fail("angr failed to load target: %s" % error)
    try:
        content = claripy.BVS("stdin", stdin_length * 8)
        state = project.factory.entry_state(
            stdin=angr.SimFile(name="stdin", content=content, has_end=True)
        )
        if params.get("printable_only", True):
            # Flags are printable ASCII, possibly NUL-terminated early. Allow
            # both so strcmp-style checks stay satisfiable.
            for byte in range(stdin_length):
                symbol = content.get_byte(byte)
                state.solver.add(
                    claripy.Or(
                        symbol == 0,
                        claripy.And(symbol >= 0x20, symbol <= 0x7E),
                    )
                )
        simulation = project.factory.simulation_manager(state)
        simulation.explore(find=find, avoid=avoid or None, num_find=1)
    except Exception as error:  # noqa: BLE001
        fail("angr exploration failed: %s" % error)
    stats = {
        "found": len(simulation.found),
        "deadended": len(simulation.deadended),
        "errored": len(simulation.errored),
        "active": len(simulation.active),
    }
    if not simulation.found:
        fail("no state reached a find address; stats=%s" % json.dumps(stats))
    state = simulation.found[0]
    try:
        # Eval the ORIGINAL symbolic stdin against the found state's solver;
        # SimFile's internal content attribute is not a stable API.
        bytes_out = b"".join(
            state.solver.eval(content.get_byte(index), cast_to=bytes)
            for index in range(stdin_length)
        )
    except Exception as error:  # noqa: BLE001
        fail("solved a find state but could not concretize stdin: %s" % error)
    print(json.dumps({
        "solved": True,
        "input_base64": base64.b64encode(bytes_out).decode(),
        "input_hex": bytes_out.hex(),
        "length": len(bytes_out),
        "stats": stats,
    }))


if __name__ == "__main__":
    main()
