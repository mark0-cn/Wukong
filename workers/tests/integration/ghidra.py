#!/usr/bin/env python3
"""Live Ghidra worker acceptance test. Read-only headless analysis of a compiled fixture.

Run inside a worker image that has Ghidra installed (workers/Dockerfile):
  python3 /opt/wukong/tests/run.py ghidra --state /state/ghidra-acceptance

The fixture is compiled locally with gcc and Ghidra only imports/analyses it;
the target program is never executed.
"""
import argparse
import json
import subprocess
import tempfile
from pathlib import Path

from support.client import Client

FIXTURE = r'''
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static void __attribute__((noinline)) copy_input(unsigned char *dst, const unsigned char *src, size_t n) {
    memcpy(dst, src, n);
}

int main(int argc, char **argv) {
    if (argc < 2) return 2;
    FILE *file = fopen(argv[1], "rb");
    if (!file) return 3;
    unsigned char buffer[64] = {0};
    unsigned char input[128] = {0};
    size_t n = fread(input, 1, sizeof(input), file);
    fclose(file);
    copy_input(buffer, input, n);
    if (buffer[0] == 'B' && buffer[1] == 'A' && buffer[2] == 'D') abort();
    puts("fixture-ok");
    return 0;
}
'''


def find_function(client, pattern, target_sha256):
    result = client.call("ghidra.query", target="files/fixture", target_sha256=target_sha256,
                         query={"operation": "functions", "pattern": pattern, "offset": 0, "limit": 100},
                         timeout_secs=200, analysis_timeout_secs=120, max_cpu=2)
    data = result["data"]
    assert data["ok"] is True, data
    matches = [f for f in data["functions"] if f["name"] == pattern]
    assert matches, "function %r not found: %s" % (pattern, data)
    return matches[0], result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--state", required=True)
    args = parser.parse_args()
    state = Path(args.state).resolve()
    client = Client(state)
    doctor = client.call("doctor")
    client.put("fixture.c", FIXTURE.encode())
    with tempfile.TemporaryDirectory() as build:
        source_path = Path(build) / "fixture.c"
        target_path = Path(build) / "fixture"
        source_path.write_text(FIXTURE)
        subprocess.run(["gcc", "-g", "-O0", "-o", str(target_path), str(source_path)], check=True)
        uploaded = client.put("fixture", target_path.read_bytes(), executable=True)
    target_sha256 = uploaded["sha256"]
    assert target_sha256

    main_fn, functions_result = find_function(client, "main", target_sha256)
    helper_fn, _ = find_function(client, "copy_input", target_sha256)
    main_address = main_fn["address"]["value"]
    helper_address = helper_fn["address"]["value"]
    assert main_fn["address"]["space"] == "module_relative"
    assert main_fn["address"]["module_sha256"] == target_sha256

    decompiled = client.call("ghidra.query", target="files/fixture", target_sha256=target_sha256,
                             query={"operation": "decompile", "address": str(helper_address)},
                             timeout_secs=200, analysis_timeout_secs=120, max_cpu=2)
    helper_code = decompiled["data"]["code"]
    assert "memcpy" in helper_code, helper_code[:500]
    assert decompiled["data"]["artifact_path"], decompiled["data"]

    main_decompiled = client.call("ghidra.query", target="files/fixture", target_sha256=target_sha256,
                                  query={"operation": "decompile", "address": str(main_address)},
                                  timeout_secs=200, analysis_timeout_secs=120, max_cpu=2)
    code = main_decompiled["data"]["code"]
    assert ("copy_input" in code) or ("memcpy" in code), code[:500]
    assert main_decompiled["data"]["artifact_path"], main_decompiled["data"]

    xrefs = client.call("ghidra.query", target="files/fixture", target_sha256=target_sha256,
                        query={"operation": "xrefs", "address": str(helper_address), "direction": "to", "limit": 100},
                        timeout_secs=200, analysis_timeout_secs=120, max_cpu=2)
    main_end = main_address + main_fn["body_bytes"]
    assert any(ref["type"] == "UNCONDITIONAL_CALL" and main_address <= ref["from"]["value"] < main_end
               for ref in xrefs["data"]["references"]), xrefs

    instructions = client.call("ghidra.query", target="files/fixture", target_sha256=target_sha256,
                               query={"operation":"instructions", "address":str(main_address), "offset":0, "limit":2},
                               timeout_secs=200, analysis_timeout_secs=120, max_cpu=2)
    page = instructions["data"]
    assert len(page["instructions"]) == 2 and page["next_offset"] == 2, page
    assert all(row["address"]["module_sha256"] == target_sha256 and "block_start" in row for row in page["instructions"]), page
    next_page = client.call("ghidra.query", target="files/fixture", target_sha256=target_sha256,
                           query={"operation":"instructions", "address":str(main_address), "offset":2, "limit":2},
                           timeout_secs=200, analysis_timeout_secs=120, max_cpu=2)["data"]
    assert page["instructions"][-1]["address"] != next_page["instructions"][0]["address"]

    # Identical repeat queries must hit the identity-bound cache, not re-import.
    cached = client.call("ghidra.query", target="files/fixture", target_sha256=target_sha256,
                         query={"operation": "functions", "pattern": "main", "offset": 0, "limit": 100},
                         timeout_secs=200, analysis_timeout_secs=120, max_cpu=2)
    assert cached["cached"] is True, cached

    # A mismatched target hash must be rejected instead of analysed.
    try:
        client.call("ghidra.query", target="files/fixture", target_sha256="0" * 64,
                    query={"operation": "functions", "pattern": "main", "offset": 0, "limit": 100},
                    timeout_secs=200)
        raise AssertionError("mismatched target hash was not rejected")
    except AssertionError as error:
        if "error" not in str(error):
            raise

    results = {"doctor": doctor, "target_sha256": target_sha256, "functions": functions_result,
               "decompile": decompiled, "decompile_main": main_decompiled, "xrefs": xrefs, "cache": cached}
    output = state / "ghidra-acceptance-results.json"
    output.write_text(json.dumps(results, indent=2))
    print(json.dumps({"passed": True, "results": str(output), "tool_version": functions_result["tool_version"],
                      "main_address": main_address, "helper_address": helper_address}))
    client.call("worker.shutdown")
    client.close()


if __name__ == "__main__":
    main()
