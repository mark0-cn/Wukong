"""Live GDB/MI dynamic-tracing acceptance using only original trusted fixtures."""

import argparse
import base64
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import time

from support.client import Client


SOURCE = Path(__file__).resolve().parents[3] / "fixtures" / "pwn" / "src" / "trace.c"
LIMITS = {
    "max_events": 1024,
    "max_point_hits": 256,
    "max_buffer_bytes": 4096,
    "max_event_bytes": 65536,
    "max_events_bytes": 8 * 1024 * 1024,
    "stop_wait_ms": 10000,
}


def sha(data):
    return hashlib.sha256(data).hexdigest()


def point(
    point_id,
    location,
    phases=None,
    arguments=None,
    buffers=None,
    return_value=None,
    kind="function",
):
    result = {
        "kind": kind,
        "point_id": point_id,
        "location": location,
        "capture": {
            "arguments": arguments or [],
            "buffers": buffers or [],
            **({"return_value": return_value} if return_value else {}),
        },
    }
    if kind == "function":
        result["phases"] = phases or ["entry", "return"]
    return result


def symbol(module, name):
    return {"kind": "symbol", "module_sha256": module, "symbol": name}


def offset(module, value):
    return {"kind": "module_offset", "module_sha256": module, "offset": value}


def capture_leaf():
    return {
        "arguments": [{"name": "length", "index": 1, "value_type": "u64"}],
        "buffers": [
            {"name": "input", "address_argument": 0, "length": {"kind": "argument", "index": 1}},
            {"name": "output", "address_argument": 2, "length": {"kind": "argument", "index": 1}},
        ],
        "return_value": "u64",
    }


def spec(points, **limits):
    return {
        "schema_version": 1,
        "abi": "sysv_amd64",
        "points": points,
        "limits": {**LIMITS, **limits},
    }


def manifest(client, identifier, base, binary, mode, timeout=10):
    document = {
        "schema_version": 1,
        "base": "files/" + base,
        "target": "target",
        "files": {"target": sha(binary)},
        "args": [mode, "@@"],
        "cwd": ".",
        "input_mode": "argv_file",
        "env": {},
        "timeout_secs": timeout,
        "loader": None,
        "library_path": None,
    }
    return client.put("manifests/" + identifier + ".json", json.dumps(document).encode())


def start(client, launch, payload, trace_spec, request_id):
    return client.call(
        "trace.start",
        launch_spec=launch["path"],
        input=payload["path"],
        spec=trace_spec,
        request_id=request_id,
    )


def artifact_json(client, job, name):
    value = client.call("file.get", path=f"jobs/{job['job_id']}/trace/{name}")
    return json.loads(base64.b64decode(value["data_base64"]))


def artifact_events(client, job):
    value = client.call("file.get", path=f"jobs/{job['job_id']}/trace/events.jsonl")
    return [json.loads(line) for line in base64.b64decode(value["data_base64"]).splitlines()]


def symbol_offset(binary, name):
    result = subprocess.run(["nm", "-n", str(binary)], check=True, capture_output=True, text=True)
    for line in result.stdout.splitlines():
        fields = line.split()
        if len(fields) >= 3 and fields[2] == name:
            return int(fields[0], 16)
    raise AssertionError("symbol not found: " + name)


def call_offset(binary, name):
    result = subprocess.run(
        ["objdump", "-d", str(binary)], check=True, capture_output=True, text=True
    )
    in_main = False
    for line in result.stdout.splitlines():
        if line.endswith("<main>:"):
            in_main = True
            continue
        if in_main and line.endswith(":") and "<" in line and not line.startswith(" "):
            break
        if in_main and "call" in line and "<" + name + ">" in line:
            return int(line.split(":", 1)[0].strip(), 16)
    raise AssertionError("call site not found in main: " + name)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--state", type=Path, required=True)
    args = parser.parse_args()
    state = args.state.resolve()
    state.mkdir(parents=True, exist_ok=False)
    state.chmod(0o700)
    with tempfile.TemporaryDirectory() as temporary:
        build = Path(temporary)
        variants = {}
        for name, flags in [("a", []), ("b", ["-DVARIANT"]), ("nopie", ["-fno-pie", "-no-pie"])]:
            target = build / name
            subprocess.run(
                [
                    "gcc",
                    "-std=c11",
                    "-O0",
                    "-g",
                    "-fno-omit-frame-pointer",
                    *flags,
                    str(SOURCE),
                    "-o",
                    str(target),
                ],
                check=True,
            )
            variants[name] = target
        subprocess.run(["cp", str(build / "a"), str(build / "stripped")], check=True)
        subprocess.run(["strip", str(build / "stripped")], check=True)
        variants["stripped"] = build / "stripped"
        leaf_static = symbol_offset(build / "a", "leaf")
        nopie_leaf_static = symbol_offset(build / "nopie", "leaf")
        call_static = call_offset(build / "a", "leaf")
        client = Client(state)
        reports = {}
        try:
            doctor = client.call("doctor")
            assert doctor["capabilities"]["dynamic_trace_gdb"], doctor
            payload = client.put("input.bin", b"ABC")
            binaries = {
                name: client.put(f"{name}/target", target.read_bytes(), executable=True)
                for name, target in variants.items()
            }
            launches = {}

            def start_mode(name, mode, trace_spec, request_id, timeout=10):
                key = (name, mode)
                if key not in launches:
                    launches[key] = manifest(
                        client, f"{name}-{mode}", name, variants[name].read_bytes(), mode, timeout
                    )
                job = start(client, launches[key], payload, trace_spec, request_id)
                status = client.finish(job, timeout=max(10, timeout + 10))
                assert status["job_id"] == job["job_id"]
                assert not status.get("cleanup_pending"), status
                if status["state"] != "completed" or not status.get("result", {}).get(
                    "trace", {}
                ).get("complete", False):
                    print(
                        json.dumps(artifact_json(client, job, "run.json")["completion"], indent=2)
                    )
                    transcript = client.call(
                        "file.get", path=f"jobs/{job['job_id']}/trace/debugger/transcript.mi"
                    )
                    print(
                        base64.b64decode(transcript["data_base64"]).decode(errors="replace")[-4000:]
                    )
                return job, status

            leaf_capture = capture_leaf()
            basic_spec = spec(
                [
                    point("leaf", symbol(binaries["a"]["sha256"], "leaf"), **leaf_capture),
                    point("middle", symbol(binaries["a"]["sha256"], "middle"), **leaf_capture),
                ]
            )
            job, status = start_mode("a", "nested", basic_spec, "nested-a")
            assert status["state"] == "completed", status
            assert status["result"]["trace"]["complete"], status
            rows = artifact_events(client, job)
            phases = [(row.get("point_id"), row.get("phase")) for row in rows]
            assert phases.count(("leaf", "entry")) == 2, phases
            assert phases.count(("leaf", "return")) == 2, phases
            if phases.count(("middle", "entry")) != 1:
                print(json.dumps(artifact_json(client, job, "run.json")["completion"], indent=2))
                transcript = client.call(
                    "file.get", path=f"jobs/{job['job_id']}/trace/debugger/transcript.mi"
                )
                print(base64.b64decode(transcript["data_base64"]).decode(errors="replace")[-5000:])
            assert phases.count(("middle", "entry")) == 1, phases
            returns = [row for row in rows if row.get("phase") == "return"]
            assert (
                sum(
                    buffer["data_base64"] == "FBcW"
                    for row in returns
                    for buffer in row["buffers"]
                    if buffer["name"] == "output"
                )
                >= 2
            ), returns
            run = artifact_json(client, job, "run.json")
            assert run["completion"]["complete"] and run["completion"]["stream_closed"], run
            reports["nested"] = run

            replay_job, replay_status = start_mode("a", "nested", basic_spec, "nested-a")
            assert replay_job["job_id"] == job["job_id"], replay_job
            reports["replay_same_job"] = True

            variant_spec = spec(
                [
                    point("leaf", symbol(binaries["b"]["sha256"], "leaf"), **leaf_capture),
                    point("middle", symbol(binaries["b"]["sha256"], "middle"), **leaf_capture),
                ]
            )
            variant_job, variant_status = start_mode("b", "nested", variant_spec, "nested-b")
            assert variant_status["result"]["trace"]["complete"], variant_status
            variant_run = artifact_json(client, variant_job, "run.json")
            assert variant_run["plan_signature"] == run["plan_signature"]
            variant_rows = artifact_events(client, variant_job)
            assert (
                sum(
                    row.get("phase") == "entry" and row.get("point_id") == "leaf"
                    for row in variant_rows
                )
                == 1
            )
            reports["variant"] = variant_run

            recursive_spec = spec(
                [
                    point(
                        "recursive",
                        symbol(binaries["a"]["sha256"], "recursive"),
                        arguments=[{"name": "value", "index": 0, "value_type": "u32"}],
                        return_value="u32",
                    )
                ]
            )
            recursive_job, recursive_status = start_mode(
                "a", "recursive", recursive_spec, "recursive-a"
            )
            assert recursive_status["result"]["trace"]["complete"], recursive_status
            recursive_rows = artifact_events(client, recursive_job)
            entries = [row for row in recursive_rows if row.get("phase") == "entry"]
            returns = [row for row in recursive_rows if row.get("phase") == "return"]
            assert len(entries) == len(returns) == 4, recursive_rows
            assert returns[-1]["return_value"]["value"] == "24", returns[-1]
            reports["recursive"] = artifact_json(client, recursive_job, "run.json")

            stripped_spec = spec(
                [point("leaf", offset(binaries["stripped"]["sha256"], leaf_static), **leaf_capture)]
            )
            stripped_job, stripped_status = start_mode(
                "stripped", "basic", stripped_spec, "stripped-a"
            )
            assert stripped_status["result"]["trace"]["complete"], stripped_status
            stripped_run = artifact_json(client, stripped_job, "run.json")
            assert (
                stripped_run["bindings"][0]["resolved_location"]["strategy"] == "pie_initial_stop"
            ), stripped_run
            reports["stripped_pie"] = stripped_run

            nopie_spec = spec(
                [
                    point(
                        "leaf",
                        offset(binaries["nopie"]["sha256"], nopie_leaf_static - 0x400000),
                        **leaf_capture,
                    )
                ]
            )
            nopie_job, nopie_status = start_mode("nopie", "basic", nopie_spec, "nopie-a")
            assert nopie_status["result"]["trace"]["complete"], nopie_status
            reports["non_pie"] = artifact_json(client, nopie_job, "run.json")

            call_spec = spec(
                [
                    point(
                        "basic_call",
                        offset(binaries["a"]["sha256"], call_static),
                        arguments=leaf_capture["arguments"],
                        buffers=leaf_capture["buffers"],
                        kind="call_site",
                    )
                ]
            )
            call_job, call_status = start_mode("a", "basic", call_spec, "call-site-a")
            assert call_status["result"]["trace"]["complete"], call_status
            call_rows = artifact_events(client, call_job)
            assert [row.get("phase") for row in call_rows] == ["call_site", None], call_rows
            reports["call_site"] = artifact_json(client, call_job, "run.json")

            limited_job, limited_status = start_mode(
                "a",
                "loop",
                spec(
                    [point("leaf", symbol(binaries["a"]["sha256"], "leaf"), **leaf_capture)],
                    max_point_hits=4,
                ),
                "limit-a",
            )
            assert limited_status["state"] == "completed", limited_status
            assert not limited_status["result"]["trace"]["complete"], limited_status
            limited_run = artifact_json(client, limited_job, "run.json")
            assert limited_run["completion"]["limits_hit"], limited_run
            reports["limit"] = limited_run

            bad_job, bad_status = start_mode(
                "a",
                "bad",
                spec(
                    [
                        point(
                            "bad",
                            symbol(binaries["a"]["sha256"], "bad_pointer"),
                            arguments=[{"name": "length", "index": 1, "value_type": "u64"}],
                            buffers=[
                                {
                                    "name": "input",
                                    "address_argument": 0,
                                    "length": {"kind": "argument", "index": 1},
                                }
                            ],
                            return_value="u32",
                        )
                    ]
                ),
                "bad-a",
            )
            assert not bad_status["result"]["trace"]["complete"], bad_status
            bad_run = artifact_json(client, bad_job, "run.json")
            assert bad_run["completion"]["capture_errors"], bad_run
            reports["unreadable"] = bad_run

            zero_job, zero_status = start_mode(
                "a",
                "basic",
                spec(
                    [
                        point(
                            "recursive",
                            symbol(binaries["a"]["sha256"], "recursive"),
                            return_value="u32",
                        )
                    ]
                ),
                "zero-a",
            )
            assert zero_status["result"]["trace"]["complete"], zero_status
            zero_rows = artifact_events(client, zero_job)
            assert all(row.get("point_id") is None for row in zero_rows), zero_rows
            reports["zero_events"] = artifact_json(client, zero_job, "run.json")

            timeout_launch = manifest(
                client, "timeout", "a", variants["a"].read_bytes(), "hang", timeout=1
            )
            timeout_job = start(
                client,
                timeout_launch,
                payload,
                spec([point("leaf", symbol(binaries["a"]["sha256"], "leaf"), **leaf_capture)]),
                "timeout-a",
            )
            timeout_status = client.finish(timeout_job, timeout=15)
            assert timeout_status["state"] == "timed_out", timeout_status
            reports["timeout"] = artifact_json(client, timeout_job, "run.json")

            cancel_launch = manifest(
                client, "cancel", "a", variants["a"].read_bytes(), "hang", timeout=30
            )
            cancel_job = start(
                client,
                cancel_launch,
                payload,
                spec([point("leaf", symbol(binaries["a"]["sha256"], "leaf"), **leaf_capture)]),
                "cancel-a",
            )
            time.sleep(0.2)
            client.call("job.cancel", job_id=cancel_job["job_id"])
            cancel_status = client.finish(cancel_job, timeout=15)
            assert cancel_status["state"] == "cancelled", cancel_status
            assert not cancel_status.get("cleanup_pending"), cancel_status
            reports["cancel"] = artifact_json(client, cancel_job, "run.json")

            report = {
                "passed": True,
                "scope": "linux_gdb_dynamic_trace",
                "source_sha256": sha(SOURCE.read_bytes()),
                "reports": reports,
            }
            (state / "trace-results.json").write_text(json.dumps(report, indent=2))
            print(
                json.dumps(
                    {
                        "passed": True,
                        "scope": report["scope"],
                        "report": str(state / "trace-results.json"),
                    }
                )
            )
        finally:
            client.close()


if __name__ == "__main__":
    main()
