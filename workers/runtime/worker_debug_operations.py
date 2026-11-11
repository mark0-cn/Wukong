"""Typed debugger operations; model text is never an executable debugger command."""
import base64
import json
import re
import time

from worker_common import parse_address as address, bounded_int

INSPECTIONS = {"registers", "stack", "memory", "modules", "threads", "disassembly", "exception", "state"}
CONTROLS = {"run", "continue", "interrupt", "step", "next", "finish", "breakpoint_add", "breakpoint_remove", "breakpoint_enable", "breakpoint_disable"}


def inspect_request(params):
    kind = params.get("kind")
    if kind not in INSPECTIONS:
        raise ValueError("unsupported debugger inspection")
    result = {"kind": kind, "limit": bounded_int(params.get("limit"), 64, 1, 256)}
    if kind in {"memory", "disassembly"}:
        result["address"] = address(params.get("address"))
        result["length"] = bounded_int(params.get("length"), 128, 1, 65536)
        if result["address"] + result["length"] > 2**64:
            raise ValueError("debugger address range overflows")
    return result


def control_request(params):
    action = params.get("action")
    if action not in CONTROLS:
        raise ValueError("unsupported debugger control")
    result = {"action": action}
    if action == "breakpoint_add":
        location = params.get("address")
        symbol = params.get("symbol")
        if (location is None) == (symbol is None):
            raise ValueError("breakpoint requires exactly one address or symbol")
        if location is not None:
            result["address"] = address(location)
        else:
            if not isinstance(symbol, str) or len(symbol) > 512 or not re.fullmatch(r"[A-Za-z_?$][A-Za-z0-9_.$?@:!<>~]*", symbol):
                raise ValueError("breakpoint symbol must be a plain symbol name")
            result["symbol"] = symbol
    elif action in {"breakpoint_remove", "breakpoint_enable", "breakpoint_disable"}:
        identifier = params.get("breakpoint_id")
        if not isinstance(identifier, str) or not re.fullmatch(r"[0-9]{1,10}(?:\.[0-9]{1,10})?", identifier):
            raise ValueError("invalid breakpoint ID")
        result["breakpoint_id"] = identifier
    return result


def checked(session, command, timeout):
    result = session.command(command, timeout, True)
    if result.get("class") == "error":
        raise ValueError("debugger rejected operation: " + str((result.get("payload") or {}).get("msg", "unknown error"))[:2048])
    return result


def gdb_inspect(session, params, timeout):
    request = inspect_request(params)
    kind, limit = request["kind"], request["limit"]
    with session.lock:
        if kind == "state":
            payload = {"execution_state": session.execution_state}
        elif kind == "exception":
            payload = next((e.get("payload") or {} for e in reversed(session.events) if e.get("message") == "stopped"), {})
        elif kind == "registers":
            names = checked(session, "-data-list-register-names", timeout).get("payload", {}).get("register-names", [])
            selected = [(index, name) for index, name in enumerate(names) if name][:limit]
            values = checked(session, "-data-list-register-values x " + " ".join(str(index) for index, _ in selected), timeout).get("payload", {}).get("register-values", []) if selected else []
            names = dict(selected)
            payload = {"registers": [{**value, "name": names.get(int(value["number"]))} for value in values]}
        elif kind == "stack":
            payload = checked(session, f"-stack-list-frames 0 {limit-1}", timeout).get("payload")
        elif kind == "threads":
            payload = checked(session, "-thread-info", timeout).get("payload") or {}
            payload["threads"] = payload.get("threads", [])[:limit]
        elif kind == "modules":
            payload = checked(session, "-file-list-shared-libraries", timeout).get("payload") or {}
            payload["shared-libraries"] = payload.get("shared-libraries", [])[:limit]
            if session.mode != "dump":
                payload["process"] = session.profile()["process"]
        elif kind == "memory":
            payload = checked(session, f"-data-read-memory-bytes {request['address']:#x} {request['length']}", timeout).get("payload") or {}
            chunks = payload.get("memory", [])
            content = b"".join(bytes.fromhex(chunk["contents"]) for chunk in chunks)
            if len(content) > request["length"]:
                raise ValueError("debugger returned excessive memory content")
            payload = {"address": request["address"], "bytes_read": len(content), "data_base64": base64.b64encode(content).decode(), "regions": chunks}
        else:
            payload = checked(session, f"-data-disassemble -s {request['address']:#x} -e {request['address'] + request['length']:#x} -- 0", timeout).get("payload") or {}
            payload["asm_insns"] = payload.get("asm_insns", [])[:limit]
        return {"session_id": session.id, "engine": "gdb_mi", "kind": kind, "data": payload,
                "target_sha256": session.target_sha256, "dump_sha256": session.dump_sha256,
                "transcript": "debug/" + session.id + "/transcript.mi"}


def gdb_control(session, params, timeout):
    request = control_request(params)
    if session.mode == "dump":
        raise ValueError("dump sessions do not support execution control")
    action = request["action"]
    commands = {"run": "-exec-run", "continue": "-exec-continue", "interrupt": "-exec-interrupt",
                "step": "-exec-step-instruction", "next": "-exec-next-instruction", "finish": "-exec-finish"}
    if action == "breakpoint_add":
        location = f"*{request['address']:#x}" if "address" in request else request["symbol"]
        command = "-break-insert " + json.dumps(location)
    elif action.startswith("breakpoint_"):
        verb = {"breakpoint_remove": "delete", "breakpoint_enable": "enable", "breakpoint_disable": "disable"}[action]
        command = "-break-" + verb + " " + request["breakpoint_id"]
    else:
        command = commands[action]
    with session.lock:
        if action == "interrupt" and session.execution_state != "running":
            result = {"session_id":session.id, "class":"done"}
        else:
            before = session.stop_sequence
            result = checked(session, command, timeout)
            if action in {"run", "continue", "step", "next", "finish", "interrupt"}:
                interruption = None
                if not wait_for_stop(session, before, timeout):
                    checked(session, "-exec-interrupt --all", 2)
                    interruption = "deadline"
                    if not wait_for_stop(session, before, 2):
                        from worker_processes import terminate_tree
                        from worker_common import WorkerError
                        session.dead = True
                        terminate_tree(session.process)
                        session.persist("lost")
                        raise WorkerError("outcome_unknown", "GDB failed to stop after interruption; session invalidated")
                result["state"] = {"execution_state":session.execution_state, "interruption":interruption}
                result["event"] = session.last_stop
        if action == "breakpoint_add":
            result["breakpoint_id"] = (result.get("payload") or {}).get("bkpt", {}).get("number")
        return {**result, "engine": "gdb_mi", "action": action, "target_sha256": session.target_sha256}


def wait_for_stop(session, before, timeout):
    deadline = time.monotonic() + timeout
    with session.stop_condition:
        while session.stop_sequence <= before and not session.dead:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return False
            session.stop_condition.wait(min(remaining, 0.1))
        return session.stop_sequence > before
