"""Job supervisor."""
from pathlib import Path
import hashlib
import os
import platform
import threading
import time
from worker_common import artifact, atomic_json, confined, load_json, safe_env
from worker_processes import process_identity, run_process

def crash_signature(result):
    code = result.get("exit_code")
    if result.get("state") != "completed":
        return None
    if result.get("location_signature"):
        return result["location_signature"]
    if result.get("signal"):
        return "signal:" + str(result["signal"])
    if os.name == "nt" and code is not None and (code & 0xFFFFFFFF) in {0xC0000005, 0xC000001D, 0xC0000094, 0xC00000FD, 0xC0000409}:
        return "exception:" + hex(code & 0xFFFFFFFF)
    return None

def expectation_matches(result, expected):
    if result["state"] != "completed":
        return False
    if not expected:
        return crash_signature(result) is not None
    allowed = {"exit_code", "signal", "stdout_contains", "stderr_contains"}
    if set(expected) - allowed:
        raise ValueError("unsupported verification expectation")
    return all((str(value) in result[key[:-9]]) if key.endswith("_contains") else result.get(key) == value
               for key, value in expected.items())

def replay(root, directory, spec, input_bytes, index):
    target = confined(root, spec["target"], True)
    sandbox = directory / ("trial-%04d" % index)
    sandbox.mkdir()
    input_path = sandbox / "input.bin"
    input_path.write_bytes(input_bytes)
    input_path.chmod(0o444)
    experiment = None
    cwd = sandbox
    if spec.get("isolated") and not spec.get("launch_spec"):
        import isolation
        experiment = isolation.sandbox(root, isolation.allocate_uid(root))
        target = isolation.stage_file(target, experiment, "target.bin", True)
        public_input = Path(experiment["readonly"]) / "input.bin"
        public_input.write_bytes(input_bytes)
        public_input.chmod(0o444)
        input_path = public_input
        isolation.seal(experiment)
        cwd = Path(experiment["work"])
    input_mode = spec.get("input_mode", "argv")
    if input_mode not in {"argv", "stdin"}:
        raise ValueError("input_mode must be 'argv' or 'stdin'")
    if spec.get("launch_spec"):
        from worker_launch import resolve
        launch=resolve(root,spec,spec.get("isolated",False),input_path)
        experiment=launch["experiment"]
        result=run_process(launch["argv"],launch["cwd"],launch["env"],spec["timeout_secs"],spec["output_limit"],sandbox/"output",directory/"cancel",experiment,stdin_bytes=input_bytes if input_mode=="stdin" else None)
        result["launch_identity"]=launch["identity"]
    elif input_mode == "stdin":
        # Pipe the payload bytes into the target's stdin; any extra args are
        # passed through verbatim (no @@ file-path substitution).
        argv = [str(target)] + [str(arg) for arg in spec.get("args", []) if arg != "@@"]
        result = run_process(argv, cwd, safe_env(spec.get("env")), spec["timeout_secs"], spec["output_limit"], sandbox / "output", directory / "cancel", experiment, stdin_bytes=input_bytes)
    else:
        argv = [str(target)] + [str(input_path) if arg == "@@" else str(arg) for arg in spec.get("args", ["@@"])]
        result = run_process(argv, cwd, safe_env(spec.get("env")), spec["timeout_secs"], spec["output_limit"], sandbox / "output", directory / "cancel", experiment)
    result["experiment_uid"] = experiment["uid"] if experiment else None
    result["metadata_protected"] = bool(experiment)
    result["input_sha256"] = hashlib.sha256(input_bytes).hexdigest()
    result["input_mode"] = input_mode
    if spec.get("kind") == "minimize" and not spec.get("allow_signal_only", False) and crash_signature(result):
        from worker_crash import replay_snapshot
        try:
            snapshot = replay_snapshot(root, spec, sandbox / "input.bin", directory / "cancel")
        except Exception as error:
            snapshot = {"signature_strength":"unresolved", "error":str(error)[:1024]}
        atomic_json(sandbox / "crash-snapshot.json", snapshot)
        # A debugger-only failure is not evidence for the original native crash.
        if snapshot.get("signal_number") == result.get("signal"):
            result["location_signature"] = snapshot.get("location_signature")
        result["snapshot"] = artifact(root, sandbox / "crash-snapshot.json")
    result["crash_signature"] = crash_signature(result)
    result["trial"] = index
    return result

def supervise_job(root, directory):
    spec = load_json(directory / "spec.json")
    meta = load_json(directory / "status.json")
    token = spec["runner_token"]
    meta.update({"state": "running", "supervisor_pid": os.getpid(), "supervisor_identity": process_identity(os.getpid()), "started_at": time.time(), "runner_token": token})
    atomic_json(directory / "status.json", meta)
    stop = threading.Event()

    def heartbeat():
        while not stop.wait(0.5):
            atomic_json(directory / "heartbeat.json", {"runner_token": token, "pid": os.getpid(), "at": time.time()})

    atomic_json(directory / "heartbeat.json", {"runner_token": token, "pid": os.getpid(), "at": time.time()})
    threading.Thread(target=heartbeat, daemon=True).start()
    try:
        kind = spec["kind"]
        if kind in {"process", "fuzz"}:
            cwd = confined(root, spec.get("cwd", "."), True)
            argv = spec.get("argv",[])
            experiment = None
            launch=None
            if kind == 'process' and spec.get('launch_spec'):
                from worker_launch import resolve
                input_path=confined(root,spec['input'],True) if spec.get('input') else None
                launch=resolve(root,spec,spec.get('isolated',False),input_path)
                if launch['input_mode']=='pty': raise ValueError('process jobs cannot use pty')
                argv,cwd,experiment=launch['argv'],launch['cwd'],launch['experiment']
            elif spec.get("isolated"):
                import isolation
                experiment = isolation.sandbox(root, spec["execution_uid"])
                cwd = Path(experiment["work"])
                if kind == "fuzz":
                    argv = list(argv)
                    target_index = argv.index("--") + 1
                    argv[target_index] = str(isolation.stage_file(argv[target_index], experiment, "target.bin", True))
                    argv[argv.index("-i") + 1] = str(isolation.stage_directory(argv[argv.index("-i") + 1], experiment, "seeds"))
                    argv[argv.index("-o") + 1] = str(cwd / "fuzz")
                else:
                    staged = []
                    for argument in argv:
                        path = Path(argument)
                        absolute = path.resolve() if path.is_absolute() else (root / path).resolve()
                        relative_input = not path.is_absolute() and argument.replace("\\", "/").split("/", 1)[0] in {"files", "jobs", "fuzz"}
                        if (path.is_absolute() and root in absolute.parents) or relative_input:
                            if absolute.is_file():
                                argument = str(isolation.stage_file(absolute, experiment, executable=(len(staged) == 0)))
                            elif absolute.is_dir():
                                argument = str(isolation.stage_directory(absolute, experiment, "directory-" + str(len(staged))))
                            else:
                                raise ValueError("isolated jobs must write output relative to their fresh working directory")
                        staged.append(argument)
                    argv = staged
                isolation.seal(experiment)
                atomic_json(directory / "experiment.json", experiment)
            job_env = launch['env'] if launch else safe_env(spec.get("env"))
            # Jobs may persist their own outputs here; the directory belongs to
            # this job and is collected by job.artifacts. Isolated jobs cannot
            # write it and fall back to their sealed working directory.
            job_env["WUKONG_JOB_DIR"] = str(directory)
            # The state root is injected into every job environment so scripts
            # can address staged files (os.path.join(WUKONG_STATE_DIR,
            # 'files', ...)) without guessing the worker layout. It is a fixed
            # path, not a credential.
            job_env["WUKONG_STATE_DIR"] = str(root)
            stdin_bytes=None
            if kind == 'process' and spec.get('input') and (launch is None or launch['input_mode']=='stdin_pipe'):
                with (launch['stdin_path'] if launch else confined(root,spec['input'],True)).open('rb') as source: stdin_bytes=source.read(32*1024*1024+1)
                if len(stdin_bytes)>32*1024*1024: raise ValueError('process stdin exceeds 32 MiB')
            result = run_process(argv, cwd, job_env, spec["timeout_secs"], spec["output_limit"], directory / "output", directory / "cancel", experiment,stdin_bytes=stdin_bytes)
            if launch: result['launch_identity']=launch['identity']
            result["experiment_uid"] = experiment["uid"] if experiment else None
            result["metadata_protected"] = bool(experiment)
            if experiment and kind == "process":
                import isolation
                try:
                    _, paths = isolation.collect(root, experiment, directory / "artifacts", "all", safe_env())
                    result["artifacts"] = [artifact(root, path) for path in paths]
                except Exception as error:
                    result["artifact_collection_error"] = str(error)[:1024]
            meta["state"] = result["state"] if result["state"] != "completed" or result["exit_code"] == 0 else "failed"
        elif kind in {"replay", "verify"}:
            input_bytes = confined(root, spec["input"], True).read_bytes()
            results = []
            for index in range(spec.get("trials", 1)):
                if (directory / "cancel").exists():
                    break
                if spec.get("network") is not None:
                    from worker_network import replay_network
                    results.append(replay_network(root,directory,spec,input_bytes,index))
                else:
                    results.append(replay(root, directory, spec, input_bytes, index))
            successes = sum(r["network_success"] if spec.get("network") is not None else expectation_matches(r, spec.get("expected")) for r in results)
            result = {"trials": len(results), "attempts": len(results), "requested_trials": spec.get("trials", 1), "successes": successes,
                      "passed": bool(results) and len(results) == spec.get("trials", 1) and successes == len(results),
                      "observed_stage": "network_behavior" if spec.get("network") is not None else "crash" if results and all(r["crash_signature"] for r in results) else "behavior",
                      "results": results, "verifier_agent_id": spec.get("verifier_agent_id"),
                      "producer_agent_id": spec.get("producer_agent_id"), "call_id": spec.get("call_id"),
                      "target": artifact(root, confined(root, spec["target"], True)),
                      "input": artifact(root, confined(root, spec["input"], True)),
                      "expected": spec.get("expected"), "environment": {"platform": platform.platform(), "python": platform.python_version()},
                      "fresh_working_directory_per_trial": True}
            if spec.get("network") is not None:
                result.update(network_binding=spec["network_binding"], expected=spec["network"],
                              fresh_connection_per_trial=all(r["fresh_connection"] for r in results), server_reset_per_trial=False)
            meta["state"] = "cancelled" if (directory / "cancel").exists() else "completed"
        elif kind == "minimize":
            data = confined(root, spec["input"], True).read_bytes()
            deadline = time.monotonic() + spec["minimize_timeout_secs"]
            baseline = replay(root, directory, spec, data, 0)
            signature = crash_signature(baseline)
            if not signature:
                raise ValueError("input does not reproduce a crash; minimization requires a stable crash signature")
            if not spec.get("allow_signal_only", False) and not baseline.get("location_signature"):
                raise ValueError("location signature unavailable; inspect trial-0000/crash-snapshot.json. Install GDB/pygdbmi or explicitly opt into allow_signal_only (weak crash equivalence).")
            from worker_minimize import reduce_input
            def equivalent(candidate, index):
                observed = replay(root, directory, spec, candidate, index)
                if observed.get("state") != "completed":
                    return None
                if (not spec.get("allow_signal_only", False)
                        and crash_signature(observed) and not observed.get("location_signature")):
                    return None
                return crash_signature(observed) == signature
            data, reduction = reduce_input(data, equivalent,
                max_attempts=spec["max_attempts"], deadline=deadline,
                cancelled=lambda: (directory / "cancel").exists())
            minimized = directory / "minimized.bin"
            minimized.write_bytes(data)
            result = {"artifact": artifact(root, minimized), "original_size": confined(root, spec["input"], True).stat().st_size,
                      "minimized_size": len(data), "crash_signature": signature, **reduction,
                      "signature_strength": "signal_only" if spec.get("allow_signal_only", False) else "location_and_frames"}
            meta["state"] = "cancelled" if (directory / "cancel").exists() else "completed"
        else:
            raise ValueError("unknown job kind")
        meta["result"] = result
    except Exception as error:
        meta.update({"state": "failed", "error": str(error)[:4096]})
    finally:
        stop.set()
        meta["finished_at"] = time.time()
        atomic_json(directory / "status.json", meta)
