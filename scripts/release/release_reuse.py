"""Find trusted exact-commit CI and successful client builds; otherwise build normally."""
import json
import os
import re
import subprocess
from scripts.release.client_artifact import TARGETS
from scripts._lib.github import api
from scripts.release.tag_release import validate_ci


def trusted_run(run, repository, sha):
    if (run.get("head_sha") != sha or run.get("status") != "completed"
            or run.get("repository", {}).get("full_name") != repository
            or run.get("head_repository", {}).get("full_name") != repository):
        return False
    path, event, branch = (run.get(key) for key in ("path", "event", "head_branch"))
    if path == ".github/workflows/ci.yml":
        return event in ("push", "workflow_dispatch") and branch == "master"
    if path == ".github/workflows/release.yml":
        return ((event == "workflow_dispatch" and branch == "master")
                or (event == "push" and bool(re.fullmatch(r"v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", branch or ""))))
    return False


def selected_artifact(job_name, artifact_name, jobs, artifacts):
    matches = [job for job in jobs if job.get("name") == job_name]
    if len(matches) != 1 or matches[0].get("conclusion") != "success":
        return None
    job = matches[0]
    # A stale artifact from an earlier failed attempt cannot stand in for this build.
    matches = [a for a in artifacts if a.get("name") == artifact_name and not a.get("expired", True)
               and a.get("size_in_bytes", 0) > 0
               and job.get("started_at", "~") <= a.get("created_at", "") <= job.get("completed_at", "")
               ]
    return matches[0] if len(matches) == 1 else None


def successful_artifact(target, jobs, artifacts):
    return selected_artifact(target + "-client / build", TARGETS[target][0], jobs, artifacts) is not None


def empty_result():
    return dict(ci_run="", **{target.replace("-", "_") + "_run": "" for target in TARGETS},
                windows_installer_id="", linux_installer_id="", windows_installer_run="", linux_installer_run="")


def discover(repository, sha, current_run, request=api):
    if not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise ValueError("Expected exact source SHA")
    result = empty_result()
    runs = request(f"repos/{repository}/actions/runs?head_sha={sha}&per_page=100")["workflow_runs"]
    for run in runs:
        if str(run["id"]) == str(current_run):
            if run.get("run_attempt", 1) <= 1:
                continue
            # Re-run all jobs can retain clients from the immediately previous
            # completed attempt. Never consume output from this running attempt.
            run = request(f"repos/{repository}/actions/runs/{run['id']}/attempts/{run['run_attempt'] - 1}")
        if not trusted_run(run, repository, sha):
            continue
        endpoint = f"repos/{repository}/actions/runs/{run['id']}"
        data = request(endpoint + f"/attempts/{run['run_attempt']}/jobs?per_page=100")
        jobs = data["jobs"]
        if len(jobs) != data["total_count"]:
            continue
        if not result["ci_run"]:
            try:
                validate_ci(run, jobs, repository)
                result["ci_run"] = str(run["id"])
            except ValueError:
                pass
        if not all(result.values()):
            artifacts = request(endpoint + "/artifacts?per_page=100")["artifacts"]
            for target in TARGETS:
                if not result[target.replace("-", "_") + "_run"] and successful_artifact(target, jobs, artifacts):
                    result[target.replace("-", "_") + "_run"] = str(run["id"])
            for target in ("windows", "linux"):
                key = target + "_installer_id"
                artifact = selected_artifact(target + "-installer / build", target + "-installer-v1", jobs, artifacts)
                if not result[key] and artifact and isinstance(artifact.get("id"), int) and artifact["id"] > 0:
                    result[key] = str(artifact["id"])
                    result[target + "_installer_run"] = str(run["id"])
        if all(result.values()):
            break
    return result


def main():
    try:
        result = discover(os.environ["GITHUB_REPOSITORY"], os.environ["GITHUB_SHA"], os.environ["GITHUB_RUN_ID"])
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        # API failure must not weaken validation. Full CI and local builds are the fallback.
        print(f"::warning::Reuse discovery unavailable ({type(error).__name__}); running full checks/builds")
        result = empty_result()
    with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as stream:
        for key, value in result.items():
            stream.write(f"{key}={value}\n")
    print(json.dumps(result))


if __name__ == "__main__":
    main()
