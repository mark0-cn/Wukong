import json
import threading
import time
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import patch

from wukong_worker.debugger.trace import TraceDriver, console_start, typed_value


LIMITS = {
    "max_events": 1024,
    "max_point_hits": 256,
    "max_buffer_bytes": 4096,
    "max_event_bytes": 65536,
    "max_events_bytes": 1048576,
    "stop_wait_ms": 10000,
}
PROFILE = {
    "pie": False,
    "image_base": 0x400000,
    "segments": [{"vaddr": 0x400000, "offset": 0, "filesz": 0x10000, "memsz": 0x10000, "flags": 1}],
}


def point(kind="function", phases=None, buffers=None, return_value="u32"):
    return {
        "kind": kind,
        "point_id": "leaf",
        "location": {"kind": "module_offset", "module_sha256": "a" * 64, "offset": 0x100},
        **({"phases": phases or ["entry", "return"]} if kind == "function" else {}),
        "capture": {
            "arguments": [{"name": "input", "index": 0, "value_type": "u64"}],
            "buffers": buffers or [],
            "return_value": return_value,
        },
    }


def spec(points):
    return {"schema_version": 1, "abi": "sysv_amd64", "points": points, "limits": LIMITS}


class FakeSession:
    def __init__(self, stops, register_frames=None, memory=None):
        self.stops = list(stops)
        self.register_frames = list(register_frames or [])
        self.memory = dict(memory or {})
        self.current_registers = (
            self.register_frames.pop(0)
            if self.register_frames
            else {
                "rdi": 0x5000,
                "rsi": 4,
                "rax": 7,
                "rsp": 0x6000,
            }
        )
        self.breakpoint = 0
        self.stop_sequence = 0
        self.last_stop = None
        self.execution_state = "stopped"
        self.stop_condition = threading.Condition()
        self.lock = threading.RLock()
        self.dead = False
        self.mode = "launch"
        self.isolated = False
        self.id = "fake"
        self.transcript = "fake"
        self.stdin_path = None
        self.target_path = "unused"
        self.source_target_sha256 = "a" * 64
        self.executed_target_sha256 = "a" * 64
        self.target_sha256 = self.executed_target_sha256
        self.dump_sha256 = None

    def next_stop(self):
        self.stop_sequence += 1
        self.last_stop = self.stops.pop(0)
        if self.stop_sequence > 1 and self.register_frames:
            self.current_registers = self.register_frames.pop(0)
        return {"class": "done", "payload": {}, "event": self.last_stop}

    def command(self, command, timeout, internal=False):
        if command.startswith("-break-insert"):
            self.breakpoint += 1
            return {
                "class": "done",
                "payload": {"bkpt": {"number": str(self.breakpoint)}},
                "breakpoint_id": str(self.breakpoint),
            }
        if command.startswith("-data-list-register-names"):
            names = ["rdi", "rsi", "rdx", "rcx", "r8", "r9", "rax", "rsp"]
            return {"class": "done", "payload": {"register-names": names}}
        if command.startswith("-data-list-register-values"):
            names = ["rdi", "rsi", "rdx", "rcx", "r8", "r9", "rax", "rsp"]
            indices = [int(value) for value in command.split()[2:]]
            return {
                "class": "done",
                "payload": {
                    "register-values": [
                        {
                            "number": str(index),
                            "value": hex(self.current_registers.get(names[index], 0)),
                        }
                        for index in indices
                    ]
                },
            }
        if command.startswith("-data-read-memory-bytes"):
            address = int(command.split()[1], 16)
            length = int(command.split()[2])
            content = self.memory.get(address, b"\0" * length)[:length]
            return {
                "class": "done",
                "payload": {
                    "memory": [
                        {
                            "addr": hex(address),
                            "contents": content.hex(),
                        }
                    ]
                },
            }
        if command.startswith("-stack-list-frames"):
            return {
                "class": "done",
                "payload": {
                    "stack": [
                        {"level": "0", "addr": "0x400100", "func": "leaf"},
                        {
                            "level": "1",
                            "addr": hex(self.current_registers.get("caller", 0x401000)),
                            "func": "middle",
                        },
                    ]
                },
            }
        if command in {"-exec-run", "-exec-continue", "-exec-finish"}:
            return self.next_stop()
        raise AssertionError("unexpected fake GDB command: " + command)


class TraceDriverTests(unittest.TestCase):
    def test_console_start_preserves_argv_with_stdin_redirection(self):
        session = FakeSession([])
        session.launch = {"args": ["null", "with space", "<literal>"]}
        session.stdin_path = "/tmp/input file"
        self.assertEqual(
            console_start(session),
            "run null 'with space' '<literal>' < '/tmp/input file'",
        )
        self.assertEqual(
            console_start(session, starti=True),
            "starti null 'with space' '<literal>' < '/tmp/input file'",
        )

    def test_typed_values_preserve_width_and_sign(self):
        self.assertEqual(typed_value(0xFFFFFFFF, "u32")["value"], "4294967295")
        self.assertEqual(typed_value(0xFFFFFFFF, "i32")["value"], "-1")
        self.assertEqual(typed_value(0xFF, "i8")["value"], "-1")
        self.assertEqual(typed_value(0x80000000, "pointer")["value"], "0x80000000")

    def test_entry_return_and_saved_buffer(self):
        buffer_spec = {
            "name": "bytes",
            "address_argument": 0,
            "length": {"kind": "argument", "index": 1},
        }
        session = FakeSession(
            [
                {"reason": "breakpoint-hit", "bkptno": "1", "thread-id": "1"},
                {"reason": "function-finished", "thread-id": "1", "frame": {"addr": "0x401000"}},
                {"reason": "exited-normally", "thread-id": "1"},
            ],
            register_frames=[
                {"rdi": 0x5000, "rsi": 4, "rax": 0, "rsp": 0x6000},
                {"rdi": 0x9999, "rsi": 0, "rax": 7, "rsp": 0x7000},
            ],
            memory={0x6000: (0x401000).to_bytes(8, "little"), 0x5000: b"test"},
        )
        with (
            TemporaryDirectory() as directory,
            patch("wukong_worker.debugger.trace.elf_profile", return_value=PROFILE),
        ):
            driver = TraceDriver(
                session,
                spec([point(buffers=[buffer_spec])]),
                Path(directory) / "events.jsonl",
                Path(directory) / "cancel",
                time.monotonic() + 10,
            )
            result = driver.run()
            stream = driver.close()
            rows = [
                json.loads(line)
                for line in (Path(directory) / "events.jsonl").read_text().splitlines()
            ]
        self.assertTrue(result["complete"], result)
        self.assertEqual([row["phase"] for row in rows[:2]], ["entry", "return"])
        self.assertEqual(rows[0]["call_id"], rows[1]["call_id"])
        self.assertEqual(rows[1]["return_value"]["value"], "7")
        self.assertEqual(stream["events_written"], 3)
        self.assertEqual(rows[1]["buffers"][0]["requested_bytes"], 4)

    def test_recursive_returns_use_activation_stack(self):
        session = FakeSession(
            [
                {"reason": "breakpoint-hit", "bkptno": "1", "thread-id": "1"},
                {"reason": "breakpoint-hit", "bkptno": "1", "thread-id": "1"},
                {"reason": "function-finished", "thread-id": "1", "frame": {"addr": "0x402000"}},
                {"reason": "function-finished", "thread-id": "1", "frame": {"addr": "0x401000"}},
                {"reason": "exited-normally", "thread-id": "1"},
            ],
            register_frames=[
                {"rdi": 1, "rsi": 0, "rax": 0, "rsp": 0x6000},
                {"rdi": 2, "rsi": 0, "rax": 0, "rsp": 0x6008, "caller": 0x402000},
                {"rdi": 0, "rsi": 0, "rax": 2, "rsp": 0x6010},
                {"rdi": 0, "rsi": 0, "rax": 1, "rsp": 0x6018},
            ],
            memory={
                0x6000: (0x401000).to_bytes(8, "little"),
                0x6008: (0x402000).to_bytes(8, "little"),
            },
        )
        with (
            TemporaryDirectory() as directory,
            patch("wukong_worker.debugger.trace.elf_profile", return_value=PROFILE),
        ):
            driver = TraceDriver(
                session,
                spec([point(buffers=[])]),
                Path(directory) / "events.jsonl",
                Path(directory) / "cancel",
                time.monotonic() + 10,
            )
            result = driver.run()
            driver.close()
            rows = [
                json.loads(line)
                for line in (Path(directory) / "events.jsonl").read_text().splitlines()
            ]
        self.assertTrue(result["complete"], result)
        self.assertEqual([row["call_id"] for row in rows[:4]], [0, 1, 1, 0])
        self.assertEqual(rows[1]["parent_call_id"], 0)
        self.assertEqual(result["open_call_ids"], [])

    def test_second_thread_is_partial(self):
        session = FakeSession(
            [
                {"reason": "breakpoint-hit", "bkptno": "1", "thread-id": "1"},
                {"reason": "breakpoint-hit", "bkptno": "1", "thread-id": "2"},
            ],
            register_frames=[
                {"rdi": 1, "rsi": 0, "rax": 0, "rsp": 0x6000},
                {"rdi": 2, "rsi": 0, "rax": 0, "rsp": 0x6008, "caller": 0x402000},
            ],
            memory={
                0x6000: (0x401000).to_bytes(8, "little"),
                0x6008: (0x402000).to_bytes(8, "little"),
            },
        )
        with (
            TemporaryDirectory() as directory,
            patch("wukong_worker.debugger.trace.elf_profile", return_value=PROFILE),
        ):
            driver = TraceDriver(
                session,
                spec([point(buffers=[])]),
                Path(directory) / "events.jsonl",
                Path(directory) / "cancel",
                time.monotonic() + 10,
            )
            result = driver.run()
            driver.close()
        self.assertFalse(result["complete"])
        self.assertIn("multi_thread_unsupported", result["capture_errors"])


if __name__ == "__main__":
    unittest.main()
