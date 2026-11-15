"""Owned job execution policy; returns observations without publishing job state."""

from pathlib import Path
from wukong_worker.artifacts.common import artifact, confined
from wukong_worker.artifacts.metadata import atomic_json
from wukong_worker.platform.environment import safe_env
from wukong_worker.execution.processes import run_process
from wukong_worker.core.jobs import JobOutcome


def execute(root, directory, spec):
    kind = spec["kind"]
    cwd = confined(root, spec.get("cwd", "."), True)
    argv = spec.get("argv", [])
    experiment = None
    launch = None
    if kind == "process" and spec.get("launch_spec"):
        from wukong_worker.execution.launch import resolve

        input_path = confined(root, spec["input"], True) if spec.get("input") else None
        launch = resolve(root, spec, spec.get("isolated", False), input_path)
        if launch["input_mode"] == "pty":
            raise ValueError("process jobs cannot use pty")
        argv, cwd, experiment = launch["argv"], launch["cwd"], launch["experiment"]
    elif spec.get("isolated"):
        import wukong_worker.platform.isolation as isolation

        experiment = isolation.sandbox(root, spec["execution_uid"])
        cwd = Path(experiment["work"])
        if kind == "fuzz":
            argv = list(argv)
            target_index = argv.index("--") + 1
            argv[target_index] = str(
                isolation.stage_file(argv[target_index], experiment, "target.bin", True)
            )
            argv[argv.index("-i") + 1] = str(
                isolation.stage_directory(argv[argv.index("-i") + 1], experiment, "seeds")
            )
            argv[argv.index("-o") + 1] = str(cwd / "fuzz")
        else:
            from wukong_worker.execution.staging import stage_argv

            argv = stage_argv(root, argv, experiment, spec.get("executable_inputs", []))
        isolation.seal(experiment)
        atomic_json(directory / "experiment.json", experiment)
    job_env = launch["env"] if launch else safe_env(spec.get("env"))
    # Jobs may persist their own outputs here; the directory belongs to
    # this job and is collected by job.artifacts. Isolated jobs cannot
    # write it and fall back to their sealed working directory.
    job_env["WUKONG_JOB_DIR"] = str(directory)
    # The state root is injected into every job environment so scripts
    # can address staged files (os.path.join(WUKONG_STATE_DIR,
    # 'files', ...)) without guessing the worker layout. It is a fixed
    # path, not a credential.
    job_env["WUKONG_STATE_DIR"] = str(root)
    stdin_bytes = None
    if (
        kind == "process"
        and spec.get("input")
        and (launch is None or launch["input_mode"] == "stdin_pipe")
    ):
        with (launch["stdin_path"] if launch else confined(root, spec["input"], True)).open(
            "rb"
        ) as source:
            stdin_bytes = source.read(32 * 1024 * 1024 + 1)
        if len(stdin_bytes) > 32 * 1024 * 1024:
            raise ValueError("process stdin exceeds 32 MiB")
    result = run_process(
        argv,
        cwd,
        job_env,
        spec["timeout_secs"],
        spec["output_limit"],
        directory / "output",
        directory / "cancel",
        experiment,
        stdin_bytes=stdin_bytes,
    )
    if launch:
        result["launch_identity"] = launch["identity"]
        result["source_target_sha256"] = launch["source_target_sha256"]
        result["executed_target_sha256"] = launch["executed_target_sha256"]
        result["input_mode"] = launch["input_mode"]
    from wukong_worker.execution.staging import execution_metadata

    result["execution"] = execution_metadata(cwd, experiment)
    result["experiment_uid"] = experiment["uid"] if experiment else None
    result["metadata_protected"] = bool(experiment)
    if experiment and kind == "process":
        import wukong_worker.platform.isolation as isolation

        try:
            _, paths = isolation.collect(
                root, experiment, directory / "artifacts", "all", safe_env()
            )
            result["artifacts"] = [artifact(root, path) for path in paths]
        except Exception as error:
            result["artifact_collection_error"] = str(error)[:1024]
    state = (
        result["state"] if result["state"] != "completed" or result["exit_code"] == 0 else "failed"
    )
    return JobOutcome(state, result)
