"""AFL statistics and crash artifacts projected onto a generic job observation."""

from wukong_worker.artifacts.common import artifact, confined
from wukong_worker.artifacts.metadata import load_json
from wukong_worker.platform.environment import safe_env


def observe(root, meta):
    if meta["kind"] != "fuzz":
        return meta
    directory = root / "jobs" / meta["job_id"]
    spec = load_json(directory / "spec.json")
    out = confined(root, spec["afl_output"])
    stats = {}
    isolated_crashes = None
    if spec.get("isolated") and (directory / "experiment.json").is_file():
        import wukong_worker.platform.isolation as isolation

        try:
            experiment = load_json(directory / "experiment.json")
            stats, paths = isolation.collect(
                root, experiment, directory / "fuzz-artifacts", "fuzz", safe_env()
            )
            isolated_crashes = [artifact(root, path) for path in paths]
        except Exception as error:
            meta["artifact_collection_error"] = str(error)[:1024]
    for path in sorted(out.glob("*/fuzzer_stats"))[:32]:
        stats[path.parent.name] = dict(
            line.split(":", 1)[0].strip()
            and [line.split(":", 1)[0].strip(), line.split(":", 1)[1].strip()]
            for line in path.read_text(errors="replace").splitlines()
            if ":" in line
        )
    meta["fuzzer_stats"] = stats
    meta["crashes"] = (
        isolated_crashes
        if isolated_crashes is not None
        else [
            artifact(root, path)
            for path in sorted(out.glob("*/crashes/id*"))[:128]
            if path.is_file()
        ]
    )
    meta["crash_reporting_override"] = (
        spec.get("env", {}).get("AFL_I_DONT_CARE_ABOUT_MISSING_CRASHES") == "1"
    )
    meta["crash_collection_disabled"] = spec.get("env", {}).get("AFL_SKIP_CRASHES") == "1"
    return meta
