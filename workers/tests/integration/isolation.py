#!/usr/bin/env python3
"""Adversarial fixture: prove targets cannot forge metadata or cross-job files."""
import json
from pathlib import Path
import sys
import time
from support.client import Client
from worker_processes import process_identity


def cancellation_proof(client,root):
    # Both the isolated leader and its descendant ignore SIGTERM. A terminal
    # job receipt must therefore be backed by SIGKILL of the whole owned group.
    fixture=("import os,signal,subprocess,sys,time,json,pathlib;"
             "signal.signal(signal.SIGTERM,signal.SIG_IGN);"
             "child=subprocess.Popen([sys.executable,'-c','import time;time.sleep(60)']);"
             "pathlib.Path('cancellation-ready.tmp').write_text(json.dumps({'leader':os.getpid(),'child':child.pid,'uid':os.getuid()}));"
             "os.replace('cancellation-ready.tmp','cancellation-ready.json');"
             "time.sleep(60)")
    job=client.call('job.start',argv=[sys.executable,'-c',fixture],request_id='isolated-cancellation',timeout_secs=30)
    deadline=time.monotonic()+10
    while time.monotonic()<deadline:
        status=client.call('job.status',job_id=job['job_id'])
        experiment_path=root/'jobs'/job['job_id']/'experiment.json'
        if experiment_path.exists():
            experiment=json.loads(experiment_path.read_text())
            ready=Path(experiment['work'])/'cancellation-ready.json'
            if ready.is_file():
                observed=json.loads(ready.read_text());break
        assert status['state'] not in {'completed','failed','cancelled','timed_out','lost'},status
        time.sleep(.03)
    else:raise AssertionError('isolated cancellation fixture did not start')
    assert observed['uid']!=0,observed
    identities={pid:process_identity(pid) for pid in [observed['leader'],observed['child']]}
    assert all(identity not in {'missing','unknown'} for identity in identities.values()),identities
    client.call('job.cancel',job_id=job['job_id'])
    result=client.finish(job,timeout=10)
    assert result['state']=='cancelled',result
    def alive(pid,identity):
        if process_identity(pid)!=identity:return False
        try:return Path(f'/proc/{pid}/stat').read_text().rsplit(') ',1)[1].split()[0] not in {'Z','X'}
        except FileNotFoundError:return False
    deadline=time.monotonic()+5
    while any(alive(pid,identity) for pid,identity in identities.items()) and time.monotonic()<deadline:time.sleep(.03)
    assert not any(alive(pid,identity) for pid,identity in identities.items()),observed
    return {'job':result,'processes':observed,'identities':identities,'owned_group_dead':True}


def main():
    root = Path(sys.argv[1]).resolve()
    client = Client(root)
    doctor = client.call("doctor")
    assert doctor["metadata_protected_from_targets"] and doctor["unique_experiment_uids"], doctor
    cancellation=cancellation_proof(client,root)
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
    script = ("#!"+sys.executable+"\nimport pathlib,sys\ntry:\n pathlib.Path(" + repr(str(protected)) + ").write_text('forged')\nexcept PermissionError:\n print('metadata-write-denied');sys.exit(0)\nsys.exit(1)\n").encode()
    target = client.put("tamper-fixture.py", script, executable=True)
    input_file = client.put("input.bin", b"fixture")
    verified = client.finish(client.call("poc.verify", target=target["path"], input=input_file["path"],
                                         producer_agent_id="producer", verifier_agent_id="independent-verifier", call_id="isolation-proof", request_id="isolation-verifier",
                                         trials=2, timeout_secs=3, expected={"exit_code": 0}))
    assert verified['state']=='completed',verified
    assert verified["result"]["passed"] and not protected.exists(), verified
    uids = [trial["experiment_uid"] for trial in verified["result"]["results"]]
    assert len(set(uids)) == 2 and all(trial["metadata_protected"] for trial in verified["result"]["results"]), verified
    output = root / "isolation-results.json"
    output.write_text(json.dumps({"doctor": doctor, "cancellation":cancellation, "attacker": first, "protected_job": second, "verification": verified}, indent=2))
    print(json.dumps({"passed": True, "owned_process_group_cancelled":True,"metadata_write_denied": True, "cross_job_write_denied": True, "target_effective_capabilities": 0, "verifier_uids": uids, "results": str(output)}))
    client.call("worker.shutdown")
    client.close()


if __name__ == "__main__":
    main()
