"""Declarative, worker-owned GDB/MI function tracing."""

import base64
import io
import json
import shlex
import time
from pathlib import Path

from wukong_analysis.crash import integer
from wukong_worker.artifacts.jsonl import BoundedJsonlWriter
from wukong_worker.debugger.operations import checked, gdb_control, gdb_inspect, wait_for_stop

ARGUMENT_REGISTERS = ["rdi", "rsi", "rdx", "rcx", "r8", "r9"]
INTEGER_BITS = {
    "u8": (8, False),
    "u16": (16, False),
    "u32": (32, False),
    "u64": (64, False),
    "i8": (8, True),
    "i16": (16, True),
    "i32": (32, True),
    "i64": (64, True),
    "pointer": (64, False),
}


def elf_profile(path):
    from elftools.elf.elffile import ELFFile

    data = Path(path).read_bytes()
    elf = ELFFile(io.BytesIO(data))
    if (
        elf.get_machine_arch() not in {"x64", "x86-64"}
        or elf.elfclass != 64
        or not elf.little_endian
    ):
        raise ValueError("dynamic tracing requires a little-endian x86_64 ELF target")
    segments = [
        {
            "vaddr": int(segment["p_vaddr"]),
            "offset": int(segment["p_offset"]),
            "filesz": int(segment["p_filesz"]),
            "memsz": int(segment["p_memsz"]),
            "flags": int(segment["p_flags"]),
        }
        for segment in elf.iter_segments()
        if segment["p_type"] == "PT_LOAD"
    ]
    if not segments:
        raise ValueError("target has no loadable segments")
    return {
        "pie": elf.header.e_type == "ET_DYN",
        "image_base": min(segment["vaddr"] for segment in segments),
        "segments": segments,
    }


def executable_segment(profile, offset):
    base = profile["image_base"]
    for segment in profile["segments"]:
        start = segment["vaddr"] - base
        if segment["flags"] & 1 and start <= offset < start + segment["memsz"]:
            return segment
    raise ValueError("trace address is outside the target's executable PT_LOAD segments")


def typed_value(raw, value_type):
    value = integer(raw)
    if value is None:
        raise ValueError("debugger returned a non-integer register value")
    bits, signed = INTEGER_BITS[value_type]
    mask = (1 << bits) - 1
    narrowed = value & mask
    if signed and narrowed & (1 << (bits - 1)):
        narrowed -= 1 << bits
    return {
        "raw": f"0x{value & ((1 << 64) - 1):016x}",
        "type": value_type,
        "value": f"0x{narrowed:x}" if value_type == "pointer" else str(narrowed),
    }


def registers(session, timeout):
    payload = gdb_inspect(session, {"kind": "registers", "limit": 256}, timeout)
    return {
        item["name"]: integer(item.get("value"))
        for item in payload["data"]["registers"]
        if item.get("name") and integer(item.get("value")) is not None
    }


def read_memory(session, address, length, timeout):
    if length == 0:
        return b""
    payload = gdb_inspect(
        session, {"kind": "memory", "address": address, "length": length}, timeout
    )["data"]
    return base64.b64decode(payload["data_base64"])


def process_stop(payload):
    return payload if isinstance(payload, dict) else {}


def console_start(session, starti=False):
    """Build a CLI start command without letting redirection erase LaunchSpec argv."""
    command = "starti" if starti else "run"
    launch = getattr(session, "launch", None)
    if isinstance(launch, dict) and launch.get("args"):
        command += " " + shlex.join(launch["args"])
    if session.stdin_path:
        command += " < " + shlex.quote(str(session.stdin_path))
    return command


class TraceDriver:
    def __init__(self, session, spec, output_path, cancel_path, deadline):
        self.session = session
        self.spec = spec
        self.points = {point["point_id"]: point for point in spec["points"]}
        self.limits = spec["limits"]
        self.cancel_path = Path(cancel_path)
        self.deadline = deadline
        self.writer = BoundedJsonlWriter(output_path, self.limits["max_events_bytes"])
        self.breakpoints = {}
        self.bindings = []
        self.hits = {point_id: 0 for point_id in self.points}
        self.sequence = 0
        self.next_call_id = 0
        self.thread_sequences = {}
        self.activations = {}
        self.complete = True
        self.reason = "target_exited"
        self.limits_hit = []
        self.capture_errors = []
        self.process = None
        self.target_profile = elf_profile(session.target_path)

    def remaining(self):
        remaining = self.deadline - time.monotonic()
        if remaining <= 0:
            raise TimeoutError("trace deadline expired")
        return min(self.limits["stop_wait_ms"] / 1000, remaining)

    def execute(self, action):
        wait_deadline = min(self.deadline, time.monotonic() + self.limits["stop_wait_ms"] / 1000)
        current = action
        console = None
        if action in {"starti", "run_stdin"}:
            console = console_start(self.session, action == "starti")
        before = self.session.stop_sequence
        if console is not None:
            checked(
                self.session,
                "-interpreter-exec console " + json.dumps(console),
                self.remaining(),
            )
            current = "continue"
        while True:
            if self.cancel_path.exists():
                self.complete = False
                self.reason = "cancelled"
                return None
            tick = min(0.25, wait_deadline - time.monotonic(), self.deadline - time.monotonic())
            if tick <= 0:
                raise TimeoutError("trace stop wait or launch deadline expired")
            if console is not None:
                if wait_for_stop(self.session, before, tick):
                    return process_stop(self.session.last_stop)
                checked(self.session, "-exec-interrupt --all", 2)
                if not wait_for_stop(self.session, before, 2):
                    raise TimeoutError("GDB failed to stop after interruption")
                result = {"state": {"interruption": "deadline"}}
                console = None
            else:
                result = gdb_control(self.session, {"action": current}, tick)
            if (result.get("state") or {}).get("interruption") != "deadline":
                return process_stop(result.get("event") or self.session.last_stop)
            current = "continue"

    def add_breakpoint(self, point, address=None):
        request = {"action": "breakpoint_add"}
        if address is None:
            request["symbol"] = point["location"]["symbol"]
            strategy = "symbol"
        else:
            request["address"] = address
            strategy = "module_offset"
        result = gdb_control(self.session, request, self.remaining())
        identifier = str(result.get("breakpoint_id"))
        if identifier in {"", "None", "0"}:
            raise ValueError("GDB did not return a breakpoint identity")
        self.breakpoints[identifier] = point
        binding = {
            "point_id": point["point_id"],
            "declared_location": point["location"],
            "resolved_location": {
                "strategy": strategy,
                "breakpoint_id": identifier,
                "runtime_module_sha256": self.session.executed_target_sha256,
            },
        }
        if address is not None:
            binding["resolved_location"]["runtime_address"] = f"0x{address:x}"
            binding["resolved_location"]["module_offset"] = point["location"]["offset"]
        self.bindings.append(binding)

    def resolve_pie(self, point):
        profile = self.session.profile()["process"]
        modules = []
        identity_errors = []
        for module in profile["modules"]:
            try:
                from wukong_worker.artifacts.common import sha256_file

                candidate = (
                    Path("/proc") / str(profile["pid"]) / "root" / module["mapped_path"].lstrip("/")
                )
                if sha256_file(candidate) == self.session.executed_target_sha256:
                    modules.append(module)
            except (OSError, ValueError) as error:
                identity_errors.append(str(error)[:256])
        if len(modules) != 1:
            diagnostics = [
                {
                    "mapped_path": module.get("mapped_path"),
                    "sha256": module.get("sha256"),
                    "identity_error": module.get("identity_error"),
                }
                for module in profile["modules"][:8]
            ]
            if identity_errors:
                diagnostics.append({"read_errors": identity_errors[:4]})
            raise ValueError(
                "cannot uniquely identify the executed target mapping: "
                + json.dumps(diagnostics, separators=(",", ":"))
            )
        module = modules[0]
        offset = point["location"]["offset"]
        segment = executable_segment(self.target_profile, offset)
        base = self.target_profile["image_base"]
        candidates = []
        for mapping in profile["maps"]:
            if (
                mapping["inode"] != module["inode"]
                or mapping["device"] != module["device"]
                or "x" not in mapping["permissions"]
                or mapping["offset"] != segment["offset"]
            ):
                continue
            load_bias = mapping["start"] - segment["vaddr"]
            address = load_bias + base + offset
            if mapping["start"] <= address < mapping["end"]:
                candidates.append((load_bias, address, mapping["offset"]))
        if len(candidates) != 1:
            raise ValueError("cannot uniquely resolve a PIE trace address")
        load_bias, address, mapping_offset = candidates[0]
        self.add_breakpoint(point, address)
        self.bindings[-1]["resolved_location"].update(
            strategy="pie_initial_stop",
            load_bias=f"0x{load_bias:x}",
            mapping_offset=f"0x{mapping_offset:x}",
        )

    def prepare(self):
        deferred = []
        seen_locations = set()
        for point in self.points.values():
            location = point["location"]
            location_key = json.dumps(location, sort_keys=True)
            if location_key in seen_locations:
                raise ValueError("multiple trace points resolve to the same location")
            seen_locations.add(location_key)
            if location["kind"] == "symbol":
                self.add_breakpoint(point)
            elif self.target_profile["pie"]:
                deferred.append(point)
            else:
                executable_segment(self.target_profile, location["offset"])
                self.add_breakpoint(point, self.target_profile["image_base"] + location["offset"])
        return deferred

    def allocate_call_id(self):
        value = self.next_call_id
        self.next_call_id += 1
        return value

    def logical_thread(self, payload):
        raw = str(payload.get("thread-id", "1"))
        if raw not in self.thread_sequences:
            if self.thread_sequences:
                self.fail("multi_thread_unsupported", partial=True)
            self.thread_sequences[raw] = len(self.thread_sequences)
        return self.thread_sequences[raw]

    def fail(self, message, *, partial=False):
        self.complete = False
        self.reason = "error" if not partial else "partial"
        if message not in self.capture_errors and len(self.capture_errors) < 64:
            self.capture_errors.append(message)

    def limit(self, name):
        self.complete = False
        self.reason = "event_limit"
        if name not in self.limits_hit:
            self.limits_hit.append(name)

    def emit(
        self,
        point,
        phase,
        thread,
        call_id,
        parent_call_id,
        arguments,
        buffers,
        return_value,
        errors,
        truncated,
        process=None,
    ):
        if self.sequence >= self.limits["max_events"]:
            self.limit("max_events")
            return False
        record = {
            "schema_version": 1,
            "event": "capture" if point is not None else "process",
            "sequence": self.sequence,
            "point_id": point["point_id"] if point else None,
            "phase": phase,
            "thread_sequence": thread,
            "call_id": call_id,
            "parent_call_id": parent_call_id,
            "arguments": arguments,
            "buffers": buffers,
            "return_value": return_value,
            "capture_errors": errors[:16],
            "truncated": truncated,
            "process": process,
        }
        if len(json.dumps(record, separators=(",", ":"))) > self.limits["max_event_bytes"]:
            self.limit("max_event_bytes")
            return False
        if not self.writer.write(record):
            self.limit("max_events_bytes")
            return False
        self.sequence += 1
        for error in errors:
            if error not in self.capture_errors and len(self.capture_errors) < 64:
                self.capture_errors.append(error)
        if errors or truncated:
            self.complete = False
            if self.reason == "target_exited":
                self.reason = "partial"
        return True

    def capture_arguments(self, point, regs):
        values = {}
        result = []
        for capture in point["capture"]["arguments"]:
            index = capture["index"]
            value = regs[ARGUMENT_REGISTERS[index]]
            values[index] = value
            result.append({**capture, **typed_value(value, capture["value_type"])})
        return values, result

    def capture_buffers(self, point, saved, timeout):
        records = []
        errors = []
        truncated = False
        for capture in point["capture"]["buffers"]:
            length_spec = capture["length"]
            length = (
                length_spec["bytes"]
                if length_spec["kind"] == "constant"
                else saved[length_spec["index"]]
            )
            if length < 0 or length > 2**64 - 1:
                raise ValueError("buffer length is outside the supported range")
            address = saved[capture["address_argument"]]
            requested = min(length, self.limits["max_buffer_bytes"])
            record = {
                **capture,
                "address": {"class": "non_module", "raw": f"0x{address:x}"},
                "length_value": length,
                "requested_bytes": requested,
            }
            try:
                content = read_memory(self.session, address, requested, timeout)
                record.update(
                    bytes_read=len(content),
                    status="ok",
                    data_base64=base64.b64encode(content).decode("ascii"),
                    truncated=requested < length,
                    error=None,
                )
                if requested < length:
                    truncated = True
                    errors.append("buffer " + capture["name"] + " was truncated")
            except Exception as error:
                record.update(
                    bytes_read=0,
                    status="unreadable",
                    data_base64="",
                    truncated=False,
                    error=str(error)[:512],
                )
                errors.append("buffer " + capture["name"] + " is unreadable")
            records.append(record)
        return records, errors, truncated

    def entry_activation(self, point, thread, regs):
        parent = self.activations.get(thread, [])
        activation = {
            "call_id": self.allocate_call_id(),
            "parent_call_id": parent[-1]["call_id"] if parent else None,
            "point": point,
            "thread": thread,
        }
        saved, arguments = self.capture_arguments(point, regs)
        for capture in point["capture"]["buffers"]:
            indices = [capture["address_argument"]]
            if capture["length"]["kind"] == "argument":
                indices.append(capture["length"]["index"])
            for index in indices:
                saved[index] = regs[ARGUMENT_REGISTERS[index]]
        activation["saved"] = saved
        activation["arguments"] = arguments
        if "return" in point["phases"]:
            activation["return_address"] = None
            activation["pairing_errors"] = []
            try:
                frames = gdb_inspect(self.session, {"kind": "stack", "limit": 2}, self.remaining())[
                    "data"
                ]["stack"]
                if len(frames) > 1:
                    activation["return_address"] = integer(frames[1].get("addr"))
                else:
                    activation["pairing_errors"].append("return caller frame unavailable")
            except Exception as error:
                activation["pairing_errors"].append(
                    "return caller frame unavailable: " + str(error)[:256]
                )
        self.activations.setdefault(thread, []).append(activation)
        return activation

    def next_action(self, thread):
        stack = self.activations.get(thread, [])
        if stack and "return" in stack[-1]["point"]["phases"]:
            return "finish"
        return "continue"

    def handle_breakpoint(self, payload):
        identifier = str(payload.get("bkptno", ""))
        point = self.breakpoints.get(identifier)
        if point is None:
            self.fail("stop at an undeclared breakpoint: " + identifier)
            return None
        thread = self.logical_thread(payload)
        if self.complete is False and self.reason in {"partial", "error"}:
            return None
        point_id = point["point_id"]
        self.hits[point_id] += 1
        if self.hits[point_id] > self.limits["max_point_hits"]:
            self.limit("max_point_hits:" + point_id)
            return None
        regs = registers(self.session, self.remaining())
        if point["kind"] == "call_site":
            _, arguments = self.capture_arguments(point, regs)
            saved = {
                capture["index"]: regs[ARGUMENT_REGISTERS[capture["index"]]]
                for capture in point["capture"]["arguments"]
            }
            for capture in point["capture"]["buffers"]:
                for index in [
                    capture["address_argument"],
                    *(
                        [capture["length"]["index"]]
                        if capture["length"]["kind"] == "argument"
                        else []
                    ),
                ]:
                    saved[index] = regs[ARGUMENT_REGISTERS[index]]
            buffers, errors, truncated = self.capture_buffers(point, saved, self.remaining())
            call_id = self.allocate_call_id()
            if not self.emit(
                point,
                "call_site",
                thread,
                call_id,
                None,
                arguments,
                buffers,
                None,
                errors,
                truncated,
            ):
                return None
            return self.next_action(thread)
        activation = self.entry_activation(point, thread, regs)
        if "entry" in point["phases"]:
            buffers, errors, truncated = self.capture_buffers(
                point, activation["saved"], self.remaining()
            )
            if not self.emit(
                point,
                "entry",
                thread,
                activation["call_id"],
                activation["parent_call_id"],
                activation["arguments"],
                buffers,
                None,
                errors,
                truncated,
            ):
                return None
        if "return" in point["phases"]:
            return "finish"
        self.activations[thread].pop()
        return self.next_action(thread)

    def handle_return(self, payload):
        thread = self.logical_thread(payload)
        stack = self.activations.get(thread, [])
        if not stack:
            self.fail("function-finished without a matching activation")
            return None
        activation = stack[-1]
        point = activation["point"]
        frame = payload.get("frame") or {}
        actual = integer(frame.get("addr"))
        errors = list(activation["pairing_errors"])
        if activation["return_address"] is not None and actual != activation["return_address"]:
            errors.append("return frame does not match the entry return address")
        regs = registers(self.session, self.remaining())
        buffers, buffer_errors, truncated = self.capture_buffers(
            point, activation["saved"], self.remaining()
        )
        errors.extend(buffer_errors)
        return_type = point["capture"].get("return_value")
        return_value = typed_value(regs["rax"], return_type or "u64")
        if return_type is None:
            return_value["type"] = None
            return_value["value"] = return_value["raw"]
        if not self.emit(
            point,
            "return",
            thread,
            activation["call_id"],
            activation["parent_call_id"],
            activation["arguments"],
            buffers,
            return_value,
            errors,
            truncated,
        ):
            return None
        stack.pop()
        if not stack:
            self.activations.pop(thread, None)
        return self.next_action(thread)

    def handle(self, stop):
        reason = stop.get("reason", "")
        self.process = stop
        if reason == "breakpoint-hit":
            return self.handle_breakpoint(stop)
        if reason == "function-finished" or (not reason and any(self.activations.values())):
            return self.handle_return(stop)
        if reason.startswith("exited"):
            if any(self.activations.values()):
                self.fail("target exited with open activations", partial=True)
            self.emit(None, None, 0, None, None, [], [], None, [], False, stop)
            return None
        if reason == "signal-received":
            self.fail("target stopped by a signal", partial=True)
            self.emit(
                None, None, self.logical_thread(stop), None, None, [], [], None, [], False, stop
            )
            return None
        self.fail("unexpected debugger stop: " + str(reason), partial=True)
        self.emit(None, None, self.logical_thread(stop), None, None, [], [], None, [], False, stop)
        return None

    def run(self):
        deferred = self.prepare()
        stop = self.execute(
            "starti" if deferred else ("run_stdin" if self.session.stdin_path else "run")
        )
        if deferred:
            for point in deferred:
                self.resolve_pie(point)
            stop = self.execute("continue")
        while stop and self.complete:
            action = self.handle(stop)
            if action is None:
                break
            stop = self.execute(action)
        process = self.process or {}
        return {
            "bindings": self.bindings,
            "complete": self.complete,
            "reason": self.reason,
            "events_written": self.sequence,
            "limits_hit": self.limits_hit,
            "capture_errors": self.capture_errors,
            "open_call_ids": [
                activation["call_id"] for stack in self.activations.values() for activation in stack
            ],
            "process": process,
        }

    def close(self):
        return self.writer.close()
