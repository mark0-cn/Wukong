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
statistics, or an explicit error. The solved input is also persisted as job
artifacts (solved-input.bin, solved.json): WUKONG_JOB_DIR when the worker
provides it, otherwise the job working directory. This script runs inside the
isolated job; it never trusts the target and never touches worker metadata.

find/avoid entries are module-relative offsets as reported by the controller's
binary profile (virtual address minus the declared image base). The controller
and angr load the same file at possibly different bases, so every entry is
mapped into the address space angr actually created before exploration.
"""
import base64
import json
import os
import sys
from pathlib import Path


def fail(message):
    print(json.dumps({"solved": False, "error": str(message)[:2048]}))
    sys.exit(1)


def mapped_address(project, module_relative):
    """Translate a module-relative offset into the address space angr loaded
    the main object at. module-relative = VA - declared image base, and angr
    maps the main object at its load base (the p_vaddr base for ET_EXEC
    images, a chosen base for PIE images and Mach-O bundles), so the virtual
    address is module_relative + load base."""
    main_object = project.loader.main_object
    base = getattr(main_object, "mapped_base", None)
    if base is None:
        base = main_object.min_addr
    address = base + module_relative
    if not main_object.contains_addr(address):
        fail(
            "module-relative address 0x%x maps to 0x%x, outside the loaded "
            "image [0x%x, 0x%x); the profile image base may differ from the "
            "angr load base"
            % (module_relative, address, main_object.min_addr, main_object.max_addr + 1)
        )
    return address


def bounded_explore(simulation, find, avoid, max_steps):
    """Use angr's Explorer and native step bound; retain observed work counts."""
    steps = 0

    def observed(manager):
        nonlocal steps
        steps += 1
        return manager

    simulation.explore(find=find, avoid=avoid or [], n=max_steps, step_func=observed)
    return simulation, steps


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
    try:
        find = [
            int(item, 0) if isinstance(item, str) else int(item)
            for item in (params.get("find") or [])
        ]
        avoid = [
            int(item, 0) if isinstance(item, str) else int(item)
            for item in (params.get("avoid") or [])
        ]
    except (TypeError, ValueError):
        fail("find/avoid must be arrays of integer or 0x-hex string addresses")
    if not find:
        fail("target and a nonempty find list are required")
    if not all(0 <= item < 2**64 for item in find + avoid):
        fail("module-relative addresses must fit the 64-bit range")
    stdin_length = int(params.get("stdin_length", 64))
    max_steps = int(params.get("max_steps", 50000))
    if not (1 <= stdin_length <= 4096):
        fail("stdin_length out of range")
    if not (1 <= max_steps <= 10_000_000):
        fail("max_steps out of range")
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
        find_addresses = [mapped_address(project, offset) for offset in find]
        avoid_addresses = [mapped_address(project, offset) for offset in avoid]
        base = getattr(project.loader.main_object, "mapped_base", None)
        if base is None:
            base = project.loader.main_object.min_addr
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
        simulation, steps = bounded_explore(
            simulation, find_addresses, avoid_addresses, max_steps
        )
    except Exception as error:  # noqa: BLE001
        fail("angr exploration failed: %s" % error)
    stats = {
        "found": len(simulation.found),
        "deadended": len(simulation.deadended),
        "errored": len(simulation.errored),
        "active": len(simulation.active),
        "steps": steps,
        "step_limit_reached": not simulation.found and steps >= max_steps,
        "mapped_base": base,
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
    result = {
        "solved": True,
        "input_base64": base64.b64encode(bytes_out).decode(),
        "input_hex": bytes_out.hex(),
        "length": len(bytes_out),
        "stats": stats,
    }
    # Persist the solved input as job artifacts so the controller imports it
    # as evidence instead of scraping a bounded stdout tail. Isolated jobs
    # cannot write the worker-provided job directory and fall back to their
    # working directory, which the worker collects after the job finishes.
    for output_dir in (os.environ.get("WUKONG_JOB_DIR"), os.getcwd()):
        if not output_dir:
            continue
        try:
            Path(output_dir).mkdir(parents=True, exist_ok=True)
            (Path(output_dir) / "solved-input.bin").write_bytes(bytes_out)
            (Path(output_dir) / "solved.json").write_text(json.dumps(result), encoding="utf-8")
            break
        except OSError:
            continue
    print(json.dumps(result))


if __name__ == "__main__":
    main()
