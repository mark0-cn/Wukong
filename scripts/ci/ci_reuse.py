"""Attest and conservatively reuse ordinary PR test evidence across commit SHAs."""

import argparse
from datetime import datetime, timedelta, timezone
import io
import json
import os
from pathlib import Path
import re
import subprocess
import urllib.parse
import zipfile

from scripts._lib.github import api
from scripts.release.note_entries import FRAGMENT

SCHEMA_VERSION = 1
ARTIFACT_NAME = "ci-test-attestation-v1"
ATTESTATION_PATH = "ci-result.json"
SCOPE_KEYS = ("rust", "frontend", "linux", "apt", "uninstall", "release_clients")
JOB_NAMES = (
    "windows",
    "windows-worker",
    "linux",
    "acceptance",
    "windows-client",
    "linux-client",
    "linux-desktop-client",
)
MAX_AGE = timedelta(days=7)
SHA = re.compile(r"[0-9a-f]{40}")


def lightweight_path(path):
    return bool(
        FRAGMENT.fullmatch(path)
        or path
        in {
            "README.md",
            "README.en.md",
            "README.zh-CN.md",
            "AGENTS.md",
            "AGENTS.zh-CN.md",
            "docs/translations.json",
        }
        or (path.startswith("docs/") and path.endswith(".md"))
    )


def valid_scope(scope):
    return (
        isinstance(scope, dict)
        and set(scope) == set(SCOPE_KEYS)
        and all(scope.get(name) in ("true", "false") for name in SCOPE_KEYS)
    )


def selected_jobs(scope):
    return {
        "windows": any(scope[key] == "true" for key in ("rust", "frontend", "linux")),
        "windows-worker": scope["rust"] == "true",
        "linux": scope["linux"] == "true",
        "acceptance": any(scope[key] == "true" for key in ("apt", "uninstall")),
        **{
            name: scope["release_clients"] == "true"
            for name in ("windows-client", "linux-client", "linux-desktop-client")
        },
    }


def build_attestation(
    *,
    repository,
    run_id,
    run_attempt,
    pr_number,
    head_sha,
    merge_sha,
    tested_tree,
    scope,
    jobs,
    created_at,
):
    if not all(SHA.fullmatch(value or "") for value in (head_sha, merge_sha, tested_tree)):
        raise ValueError("Attestation requires full head, merge and tree SHAs")
    if not valid_scope(scope):
        raise ValueError("Attestation has an invalid check scope")
    results = {}
    for name in ("changes", "summary", *JOB_NAMES):
        result = jobs.get(name, {}).get("result")
        if result not in ("success", "failure", "cancelled", "skipped"):
            raise ValueError("Attestation has an invalid job result: " + name)
        results[name] = {"result": result}
    return {
        "schema_version": SCHEMA_VERSION,
        "repository": repository,
        "run_id": str(run_id),
        "run_attempt": int(run_attempt),
        "event": "pull_request",
        "pr_number": int(pr_number),
        "head_sha": head_sha,
        "merge_sha": merge_sha,
        "tested_tree": tested_tree,
        "scope": dict(scope),
        "jobs": results,
        "created_at": created_at,
    }


def write_attestation(output, environ=None, needs=None):
    environ = os.environ if environ is None else environ
    if needs is None:
        needs = json.loads(environ["NEEDS_JSON"])
    # Import locally so ci_scope can use discovery without a module cycle.
    from scripts.ci.ci_scope import checks_passed

    if environ.get("GITHUB_EVENT_NAME") != "pull_request" or not checks_passed(needs):
        raise ValueError("Only a successful ordinary PR Checks run can attest test evidence")
    outputs = needs["changes"]["outputs"]
    if outputs.get("reuse") != "false":
        raise ValueError("A reused run must not become a new evidence source")
    scope = {name: outputs.get(name) for name in SCOPE_KEYS}
    event = json.loads(Path(environ["GITHUB_EVENT_PATH"]).read_text(encoding="utf-8"))
    pr = event["pull_request"]
    merge_sha = environ.get("GITHUB_SHA", "")
    actual_merge = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
    if actual_merge != merge_sha:
        raise ValueError("Checked-out merge commit does not match GITHUB_SHA")
    tested_tree = subprocess.check_output(["git", "rev-parse", "HEAD^{tree}"], text=True).strip()
    result = build_attestation(
        repository=environ["GITHUB_REPOSITORY"],
        run_id=int(environ["GITHUB_RUN_ID"]),
        run_attempt=int(environ["GITHUB_RUN_ATTEMPT"]),
        pr_number=int(pr["number"]),
        head_sha=pr["head"]["sha"],
        merge_sha=merge_sha,
        tested_tree=tested_tree,
        scope=scope,
        jobs={**needs, "summary": {"result": "success"}},
        created_at=datetime.now(timezone.utc).isoformat(),
    )
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    return result


def read_attestation_archive(data):
    if len(data) > 128 * 1024:
        raise ValueError("CI attestation archive is too large")
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        entries = [entry for entry in archive.infolist() if not entry.is_dir()]
        if len(entries) != 1 or entries[0].filename != ATTESTATION_PATH:
            raise ValueError("Unexpected CI attestation archive contents")
        entry = entries[0]
        if entry.file_size > 64 * 1024 or entry.compress_size > 64 * 1024:
            raise ValueError("CI attestation is too large")
        with archive.open(entry) as stream:
            content = stream.read(64 * 1024 + 1)
    if len(content) > 64 * 1024:
        raise ValueError("CI attestation is too large")
    return json.loads(content.decode("utf-8"))


def download_attestation(repository, artifact_id):
    data = subprocess.check_output(
        ["gh", "api", f"repos/{repository}/actions/artifacts/{artifact_id}/zip"]
    )
    return read_attestation_archive(data)


def diff_paths(old_merge, current_merge):
    if not all(SHA.fullmatch(value) for value in (old_merge, current_merge)):
        raise ValueError("Invalid merge commit range")
    subprocess.run(
        ["git", "fetch", "--no-tags", "origin", old_merge],
        check=True,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    raw = subprocess.check_output(
        ["git", "diff", "--name-only", "--no-renames", "-z", old_merge, current_merge]
    )
    return [path.decode("utf-8", errors="replace") for path in raw.split(b"\0") if path]


def api_job_succeeded(name, jobs):
    if name == "acceptance":
        expected = "acceptance / acceptance-gate"
    elif name.endswith("-client"):
        expected = name + " / build"
    else:
        expected = name
    matches = [job for job in jobs if job.get("name") == expected]
    return len(matches) == 1 and matches[0].get("conclusion") == "success"


def equivalent_attestation(
    attestation,
    run,
    jobs,
    *,
    repository,
    pr_number,
    current_merge,
    current_tree,
    current_scope,
    delta_paths,
    now,
):
    if not isinstance(attestation, dict) or attestation.get("schema_version") != SCHEMA_VERSION:
        return None
    if run.get("status") != "completed" or run.get("conclusion") != "success":
        return None
    if (
        run.get("event") != "pull_request"
        or run.get("path") != ".github/workflows/ci.yml"
        or run.get("repository", {}).get("full_name") != repository
        or run.get("head_repository", {}).get("full_name") != repository
    ):
        return None
    expected_identity = {
        "repository": repository,
        "run_id": str(run.get("id")),
        "run_attempt": run.get("run_attempt"),
        "event": "pull_request",
        "pr_number": pr_number,
        "head_sha": run.get("head_sha"),
    }
    if any(attestation.get(key) != value for key, value in expected_identity.items()):
        return None
    if not all(SHA.fullmatch(attestation.get(key, "")) for key in ("merge_sha", "tested_tree")):
        return None
    if attestation.get("scope") != current_scope or not valid_scope(current_scope):
        return None
    try:
        created = datetime.fromisoformat(attestation["created_at"].replace("Z", "+00:00"))
        if created.tzinfo is None or not timedelta(0) <= now - created <= MAX_AGE:
            return None
    except (KeyError, TypeError, ValueError):
        return None
    results = attestation.get("jobs", {})
    for name in ("changes", "summary"):
        if results.get(name, {}).get("result") != "success" or not api_job_succeeded(name, jobs):
            return None
    selected = selected_jobs(current_scope)
    for name in JOB_NAMES:
        expected = "success" if selected[name] else "skipped"
        if results.get(name, {}).get("result") != expected:
            return None
        if selected[name] and not api_job_succeeded(name, jobs):
            return None
    if attestation["tested_tree"] == current_tree:
        return "identical tested merge tree"
    paths = delta_paths(attestation["merge_sha"], current_merge)
    if paths and all(lightweight_path(path) for path in paths):
        return "documentation-only delta from a successful PR run"
    return None


def discover(
    *,
    repository,
    pr_number,
    head_branch,
    current_run,
    current_merge,
    current_tree,
    current_scope,
    request=api,
    downloader=download_attestation,
    delta_paths=diff_paths,
    now=None,
):
    now = datetime.now(timezone.utc) if now is None else now
    query = urllib.parse.urlencode({"event": "pull_request", "branch": head_branch, "per_page": 20})
    runs = request(f"repos/{repository}/actions/workflows/ci.yml/runs?{query}")["workflow_runs"]
    for run in runs:
        if str(run.get("id")) == str(current_run):
            continue
        try:
            attempt = int(run["run_attempt"])
            endpoint = f"repos/{repository}/actions/runs/{run['id']}"
            job_page = request(endpoint + f"/attempts/{attempt}/jobs?per_page=100")
            jobs = job_page["jobs"]
            if len(jobs) != job_page["total_count"]:
                continue
            artifact_page = request(endpoint + "/artifacts?per_page=100")
            artifacts = artifact_page["artifacts"]
            if len(artifacts) != artifact_page["total_count"]:
                continue
            summary = [job for job in jobs if job.get("name") == "summary"]
            if len(summary) != 1:
                continue
            summary = summary[0]
            matches = [
                artifact
                for artifact in artifacts
                if artifact.get("name") == ARTIFACT_NAME
                and not artifact.get("expired", True)
                and 0 < artifact.get("size_in_bytes", 0) <= 128 * 1024
                and summary.get("started_at", "~")
                <= artifact.get("created_at", "")
                <= summary.get("completed_at", "")
            ]
            if len(matches) != 1:
                continue
            attestation = downloader(repository, matches[0]["id"])
            reason = equivalent_attestation(
                attestation,
                run,
                jobs,
                repository=repository,
                pr_number=pr_number,
                current_merge=current_merge,
                current_tree=current_tree,
                current_scope=current_scope,
                delta_paths=delta_paths,
                now=now,
            )
            if reason:
                return {
                    "reuse": True,
                    "reuse_source_run": str(run["id"]),
                    "reuse_reason": reason,
                }
        except (
            OSError,
            ValueError,
            KeyError,
            TypeError,
            subprocess.CalledProcessError,
            zipfile.BadZipFile,
        ):
            continue
    return {
        "reuse": False,
        "reuse_source_run": "",
        "reuse_reason": "No verified equivalent PR checks",
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    write = subparsers.add_parser("write")
    write.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.command == "write":
        result = write_attestation(args.output)
        print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
