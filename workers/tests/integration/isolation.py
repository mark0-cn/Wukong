#!/usr/bin/env python3
"""Adversarial fixture: prove targets cannot forge metadata or cross-job files."""
import json
from pathlib import Path
import sys
import time
from support.client import Client


def main():
    root = Path(sys.argv[1]).resolve()
    client = Client(root)
    doctor = client.call("doctor")
    assert doctor["metadata_protected_from_targets"] and doctor["unique_experiment_uids"], doctor
    protected = root / "forged-status.txt"
    second = client.call("job.start", argv=[sys.executable, "-c", "import pathlib,time;pathlib.Path('guard').write_text('original');time.sleep(3)"], request_id="protected-job", timeout_secs=10)
    for _ in range(100):
        path = root / "jobs" / second["job_id"] / "experiment.json"
        if path.exists():
            second_experiment = json.loads(path.read_text())
            guard = Path(second_experiment["work"]) / "guard"
            if guard.is_file():
                break
        time.sleep(0.03)
    else:
        raise AssertionError("second experiment did not start")
    attack = """import json,os,pathlib
denied=[]
for path in PATHS:
    try:
        pathlib.Path(path).write_text('forged')
    except PermissionError:
        denied.append(path)
print(json.dumps({'uid':os.getuid(),'denied':denied,'cap_eff':next(x for x in pathlib.Path('/proc/self/status').read_text().splitlines() if x.startswith('CapEff:'))}))
""".replace("PATHS", repr([str(protected), str(guard)]))
    first = client.finish(client.call("job.start", argv=[sys.executable, "-c", attack], request_id="adversarial-job", timeout_secs=5))
    assert first["state"] == "completed", first
    observed = json.loads(first["result"]["stdout"])
    assert len(observed["denied"]) == 2, observed
    assert observed["cap_eff"].endswith("0000000000000000"), observed
    assert observed["uid"] != 0 and observed["uid"] != second_experiment["uid"], observed
    assert guard.read_text() == "original" and not protected.exists()
    second = client.finish(second)
    assert second["state"] == "completed", second
    # Verification uses the same privilege boundary and independently records
    # kernel exit status. A target can emit data, but cannot edit verdict files.
    script = ("#!/usr/bin/python3\nimport pathlib,sys\ntry:\n pathlib.Path(" + repr(str(protected)) + ").write_text('forged')\nexcept PermissionError:\n print('metadata-write-denied');sys.exit(0)\nsys.exit(1)\n").encode()
    target = client.put("tamper-fixture.py", script, executable=True)
    input_file = client.put("input.bin", b"fixture")
    verified = client.finish(client.call("poc.verify", target=target["path"], input=input_file["path"],
                                         producer_agent_id="producer", verifier_agent_id="independent-verifier", call_id="isolation-proof", request_id="isolation-verifier",
                                         trials=2, timeout_secs=3, expected={"exit_code": 0}))
    assert verified["result"]["passed"] and not protected.exists(), verified
    uids = [trial["experiment_uid"] for trial in verified["result"]["results"]]
    assert len(set(uids)) == 2 and all(trial["metadata_protected"] for trial in verified["result"]["results"]), verified
    output = root / "isolation-results.json"
    output.write_text(json.dumps({"doctor": doctor, "attacker": first, "protected_job": second, "verification": verified}, indent=2))
    print(json.dumps({"passed": True, "metadata_write_denied": True, "cross_job_write_denied": True, "target_effective_capabilities": 0, "verifier_uids": uids, "results": str(output)}))
    client.call("worker.shutdown")
    client.close()


if __name__ == "__main__":
    main()
