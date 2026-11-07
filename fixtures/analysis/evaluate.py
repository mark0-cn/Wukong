#!/usr/bin/env python3
"""Build trusted general-analysis fixtures and measure their real behavior.

This measures fixture ground truth, not agent answer quality. Keep sources,
results.json and generated build metadata out of a blind model's input.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shlex
import subprocess
import sys

ROOT = Path(__file__).resolve().parent


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def record(version, payload=b"ABC", flags=1):
    checksum = 0
    for byte in payload:
        checksum ^= byte
    width = 2 if version == 1 else 4
    return b"RECD" + bytes([version, flags]) + len(payload).to_bytes(width, "little") + payload + bytes([checksum])


def transform(value):
    mixed = (value * 1664525 + 1013904223) & 0xffffffff
    return (((mixed << 7) | (mixed >> 25)) & 0xffffffff) ^ 0xa5a5a5a5


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build-dir", type=Path, default=ROOT / "build")
    args = parser.parse_args()
    build = args.build_dir.resolve()
    build.mkdir(parents=True, exist_ok=True)
    compiler = shlex.split(os.environ.get("CC", "cc"))
    targets = {}
    for name, source, extra in [
        ("record-v1", "record.c", ["-DRECORD_VERSION=1"]),
        ("record-v2", "record.c", ["-DRECORD_VERSION=2"]),
        ("transform", "transform.c", []),
        ("session", "session.c", []),
    ]:
        target = build / (name + (".exe" if os.name == "nt" else ""))
        command = compiler + ["-std=c11", "-O0", "-g", "-Wall", "-Wextra", "-Werror"] + extra + [str(ROOT / source), "-o", str(target)]
        subprocess.run(command, check=True, capture_output=True, timeout=30)
        targets[name] = {"path": str(target), "sha256": sha256(target), "source_sha256": sha256(ROOT / source), "command": command}

    observations = []
    def observe(binary, label, argv, expected_code, expected_stdout, stdin=None, input_hex=None):
        result = subprocess.run([targets[binary]["path"]] + argv, input=stdin,
                                capture_output=True, timeout=3)
        passed = result.returncode == expected_code and result.stdout == expected_stdout
        observation = {"binary": binary, "case": label, "argv": argv,
                       "input_hex": input_hex, "stdin_hex": stdin.hex() if stdin is not None else None,
                       "exit_code": result.returncode, "stdout": result.stdout.decode("utf-8", "replace"),
                       "stderr": result.stderr.decode("utf-8", "replace"), "passed": passed}
        observations.append(observation)
        if not passed:
            raise AssertionError(json.dumps(observation))
        return observation

    baseline = []
    for version in [1, 2]:
        valid = record(version)
        wrong_length = bytearray(valid)
        wrong_length[6:8] = b"\x00\x03"
        cases = [
            ("valid", valid, 0, f"version={version} flags=1 length=3\n".encode()),
            ("empty", record(version, b""), 0, f"version={version} flags=1 length=0\n".encode()),
            ("long", record(version, b"A" * 300), 0, f"version={version} flags=1 length=300\n".encode()),
            ("truncated", valid[:5], 2, b"ERR truncated\n"),
            ("wrong_magic", b"FAIL" + valid[4:], 3, b"ERR format\n"),
            ("wrong_length", bytes(wrong_length), 4, b"ERR length\n"),
            ("wrong_checksum", valid[:-1] + bytes([valid[-1] ^ 1]), 5, b"ERR checksum\n"),
        ]
        for label, data, code, stdout in cases:
            sample = build / f"v{version}-{label}.bin"
            sample.write_bytes(data)
            observed = observe(f"record-v{version}", label, [str(sample)], code, stdout, input_hex=data.hex())
            if version == 1:
                baseline.append({key: observed[key] for key in ["case", "input_hex", "exit_code", "stdout"]})

    for value in [0, 1, 2, 255, 65535, 0x7fffffff, 0x80000000, 0xffffffff]:
        observe("transform", str(value), [str(value)], 0, f"{transform(value):08x}\n".encode())
    for invalid in ["-1", "4294967296", "not-a-number"]:
        observe("transform", invalid, [invalid], 2, b"")
    sequences = [
        (b"OPEN\nDATA\nCOMMIT\nQUIT\n", b"READY\nOK OPEN\nOK DIRTY\nOK DONE\nBYE\n"),
        (b"DATA\nOPEN\nCOMMIT\nQUIT\n", b"READY\nERR state\nOK OPEN\nERR state\nBYE\n"),
        (b"OPEN\nDATA\nRESET\nCOMMIT\nQUIT\n", b"READY\nOK OPEN\nOK DIRTY\nOK IDLE\nERR state\nBYE\n"),
        (b"OPEN\nOPEN\nDATA\nDATA\nCOMMIT\nDATA\nQUIT\n", b"READY\nOK OPEN\nERR state\nOK DIRTY\nOK DIRTY\nOK DONE\nERR state\nBYE\n"),
    ]
    for index, (stdin, stdout) in enumerate(sequences):
        observe("session", f"sequence-{index}", [], 0, stdout, stdin=stdin)

    manifest = {"schema_version": 1, "platform": platform.platform(), "targets": targets,
                "tasks_sha256": sha256(ROOT / "tasks.json")}
    (build / "build.json").write_text(json.dumps(manifest, indent=2) + "\n")
    (build / "baseline_profile.json").write_text(json.dumps({"target_sha256": targets["record-v1"]["sha256"], "observations": baseline}, indent=2) + "\n")
    result = {"fixture_checks_passed": len(observations), "agent_accuracy": None,
              "note": "Source-known behavior checks; agent quality requires independent report adjudication.",
              "build": manifest, "observations": observations}
    (build / "results.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"fixture_checks_passed": len(observations), "agent_accuracy": None, "build_dir": str(build)}))
    return 0


if __name__ == "__main__":
    sys.exit(main())
