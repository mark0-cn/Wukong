"""Select an already accepted tag release artifact for publication-only recovery."""
import argparse
import json
import os
import re
from scripts._lib.github import api
from scripts.release.release_reuse import selected_artifact


def select(run, jobs, artifacts, repository):
    if not (run.get("status") == "completed" and run.get("event") == "push"
            and run.get("path") == ".github/workflows/release.yml"
            and run.get("repository", {}).get("full_name") == repository
            and run.get("head_repository", {}).get("full_name") == repository
            and re.fullmatch(r"[0-9a-f]{40}", run.get("head_sha", ""))
            and re.fullmatch(r"v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", run.get("head_branch", ""))):
        raise ValueError("Expected a completed tag publication from this repository")
    for name in ("metadata", "checks", "image", "acceptance", "windows-installer / build", "linux-installer / build", "package"):
        matches = [job for job in jobs if job.get("name") == name]
        if len(matches) != 1 or matches[0].get("conclusion") != "success":
            raise ValueError("Missing successful release gate: " + name)
    artifact = selected_artifact("package", "online-release", jobs, artifacts)
    if not artifact or not isinstance(artifact.get("id"), int) or artifact["id"] <= 0:
        raise ValueError("No accepted release artifact from this attempt")
    return dict(sha=run["head_sha"], tag=run["head_branch"], artifact_id=str(artifact["id"]))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run-id", required=True, type=int)
    args = parser.parse_args()
    repository = os.environ["GITHUB_REPOSITORY"]
    base = f"repos/{repository}/actions/runs/{args.run_id}"
    run = api(base)
    jobs = api(base + f"/attempts/{run['run_attempt']}/jobs?per_page=100")
    artifacts = api(base + "/artifacts?per_page=100")
    if len(jobs["jobs"]) != jobs["total_count"] or len(artifacts["artifacts"]) != artifacts["total_count"]:
        raise ValueError("Incomplete release metadata")
    result = select(run, jobs["jobs"], artifacts["artifacts"], repository)
    with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as stream:
        for key, value in result.items():
            stream.write(f"{key}={value}\n")
    print(json.dumps(result))


if __name__ == "__main__":
    main()
