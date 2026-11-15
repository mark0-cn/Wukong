"""Dispatch the release controller only for a version-only merged App PR."""
import json
import os
import subprocess
from scripts._lib.github import api
from scripts.release.tag_release import discover_candidate, require_identity, validate_ci_source


def request_release(repository, run_id, sha, attempt):
    run = api(f"repos/{repository}/actions/runs/{run_id}")
    validate_ci_source(run, repository)
    require_identity(run, sha, attempt)
    candidate = discover_candidate(sha, repository)
    if not candidate:
        return False
    # No tag credentials here. The master-only receiver independently waits for
    # completed full CI and verifies the configured App before obtaining a token.
    payload = dict(ref="master", inputs=dict(operation="tag", ci_run_id=str(run_id),
                                            ci_sha=sha, ci_run_attempt=str(attempt)))
    subprocess.run(["gh", "api", "--method", "POST",
                    f"repos/{repository}/actions/workflows/release.yml/dispatches", "--input", "-"],
                   input=json.dumps(payload), text=True, check=True)
    return True


if __name__ == "__main__":
    requested = request_release(os.environ["GITHUB_REPOSITORY"], int(os.environ["GITHUB_RUN_ID"]),
                                os.environ["GITHUB_SHA"], int(os.environ["GITHUB_RUN_ATTEMPT"]))
    message = ("Release controller requested; it will wait for this complete Checks attempt."
               if requested else "No eligible version PR; no release workflow requested.")
    print(message)
    with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as stream:
        stream.write(message + "\n")
