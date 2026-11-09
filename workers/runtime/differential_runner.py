"""Reusable bounded stdin differential harness, executed inside the worker sandbox."""
import base64
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile

LIMIT = 65536

def file_limit():
    import resource
    resource.setrlimit(resource.RLIMIT_FSIZE, (LIMIT, LIMIT))

def observe(argv, payload, timeout):
    with tempfile.TemporaryFile() as out, tempfile.TemporaryFile() as err:
        process = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=out, stderr=err,
            start_new_session=os.name == "posix", preexec_fn=file_limit if os.name == "posix" else None)
        timed_out = False
        try:
            process.communicate(payload, timeout=timeout)
        except subprocess.TimeoutExpired:
            timed_out = True
        finally:
            if os.name == "posix":
                try: os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError: pass
            elif process.poll() is None:
                process.kill()
            process.wait()
        out.seek(0); err.seek(0)
        stdout, stderr = out.read(LIMIT), err.read(LIMIT)
        return {"exit_code":process.returncode, "timeout":timed_out,
            "truncated":len(stdout) >= LIMIT or len(stderr) >= LIMIT,
            "stdout":base64.b64encode(stdout).decode(), "stderr":base64.b64encode(stderr).decode()}

def compare(target, reference, inputs, timeout, python=False):
    if not 1 <= len(inputs) <= 64 or not 1 <= timeout <= 10:
        raise ValueError("invalid differential budget")
    rows = []
    for encoded in inputs:
        payload = base64.b64decode(encoded, validate=True)
        if len(payload) > LIMIT: raise ValueError("input too large")
        actual = observe([str(target)], payload, timeout)
        expected = observe(([sys.executable] if python else []) + [str(reference)], payload, timeout)
        complete = all(not r["timeout"] and not r["truncated"] and r["exit_code"] == 0 for r in [actual, expected])
        verdict = "inconclusive" if not complete else "match" if actual == expected else "mismatch"
        rows.append({"input_base64":encoded,"actual":actual,"expected":expected,"verdict":verdict})
    return {"target_sha256":hashlib.sha256(Path(target).read_bytes()).hexdigest(),
        "reference_sha256":hashlib.sha256(Path(reference).read_bytes()).hexdigest(),"cases":rows,
        "matches":sum(r["verdict"] == "match" for r in rows),
        "mismatches":sum(r["verdict"] == "mismatch" for r in rows),
        "inconclusive":sum(r["verdict"] == "inconclusive" for r in rows)}

if __name__ == "__main__":
    reference, raw, target = sys.argv[1:]
    config = json.loads(raw)
    result = compare(target, reference, config["inputs"], config["timeout"], config["python"])
    Path("differential-results.json").write_text(json.dumps(result), encoding="utf-8")
    print(json.dumps(result))
