"""Tag only an App-created version PR whose exact merge commit passed full master CI."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import time
from scripts._lib.github import api
from scripts._lib.versioning import ROOT, VERSION_FILES, bump_version, validate_tag, version


def git(*args, root=ROOT):
    return subprocess.check_output(["git", "-C", str(root), *args])


def validate_ci_source(run, repository):
    if not (run.get("event") == "push" and run.get("head_branch") == "master"
            and run.get("path") == ".github/workflows/ci.yml"
            and run.get("repository", {}).get("full_name") == repository
            and run.get("head_repository", {}).get("full_name") == repository):
        raise ValueError("Expected CI from a push to this repository's master")
    if not re.fullmatch(r"[0-9a-f]{40}", run.get("head_sha", "")):
        raise ValueError("Invalid CI commit SHA")


def validate_ci(run, jobs, repository):
    validate_ci_source(run, repository)
    if run.get("status") != "completed" or run.get("conclusion") != "success":
        raise ValueError("Expected completed successful CI")
    for name in ("changes", "windows", "linux", "summary"):
        matches = [j for j in jobs if j.get("name") == name]
        if len(matches) != 1 or matches[0].get("conclusion") != "success":
            raise ValueError("Required full CI job did not pass: " + name)


def release_pr(pr, sha, repository):
    head, base, author = pr.get("head", {}), pr.get("base", {}), pr.get("user", {})
    if not (pr.get("merged") is True and pr.get("merge_commit_sha") == sha
            and base.get("ref") == "master"
            and base.get("repo", {}).get("full_name") == repository
            and (head.get("repo") or {}).get("full_name") == repository
            and head.get("ref", "").startswith("codex/release-v")
            and author.get("type") == "Bot"
            and author.get("login") != "github-actions[bot]"):
        return None
    if not re.fullmatch(r"[A-Za-z0-9-]+\[bot\]", author.get("login", "")):
        raise ValueError("Invalid release App author")
    tag = head["ref"].removeprefix("codex/release-")
    validate_tag(tag)
    return dict(tag=tag, sha=sha, author=author["login"], pr=str(pr["number"]))


def snapshot(sha, destination, root):
    for name in (*VERSION_FILES, "rust-toolchain.toml"):
        path = destination / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(git("show", f"{sha}:{name}", root=root))


def validate_change(sha, tag, root=ROOT):
    validate_tag(tag)
    if not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise ValueError("Invalid merge commit SHA")
    git("merge-base", "--is-ancestor", sha, "origin/master", root=root)
    parent = git("rev-parse", sha + "^1", root=root).decode().strip()
    changed = set(git("diff", "--name-only", "--no-renames", "-z", parent, sha, root=root).decode().rstrip("\0").split("\0"))
    if changed != set(VERSION_FILES):
        raise ValueError("Release merge must change exactly the five version files")
    with tempfile.TemporaryDirectory() as directory:
        before, after = Path(directory) / "before", Path(directory) / "after"
        snapshot(parent, before, root)
        snapshot(sha, after, root)
        version(tag, root=after)
        bump_version(tag, root=before)
        for name in VERSION_FILES:
            if (before / name).read_bytes() != (after / name).read_bytes():
                raise ValueError("Release merge contains changes beyond the version bump: " + name)


def discover_candidate(sha, repository, root=ROOT):
    candidates = []
    for item in api(f"repos/{repository}/commits/{sha}/pulls?per_page=100", paginate=True):
        pr = api(f"repos/{repository}/pulls/{item['number']}")
        candidate = release_pr(pr, sha, repository)
        if candidate:
            candidates.append(candidate)
    if not candidates:
        return None
    if len(candidates) != 1:
        raise ValueError("Multiple release PRs match this merge commit")
    candidate = candidates[0]
    validate_change(candidate["sha"], candidate["tag"], root)
    return candidate


def require_identity(run, sha, attempt):
    if sha is not None and run.get("head_sha") != sha:
        raise ValueError("CI request commit changed")
    if attempt is not None and run.get("run_attempt") != attempt:
        raise ValueError("CI request attempt changed; rerun the source Checks")


def wait_for_ci(run_id, repository, sha, attempt, *, request=api, sleep=time.sleep, polls=60):
    # The dispatch job must finish before its workflow can become successful.
    # Only the receiver waits; the sender never waits for this release workflow.
    for index in range(polls):
        run = request(f"repos/{repository}/actions/runs/{run_id}")
        validate_ci_source(run, repository)
        require_identity(run, sha, attempt)
        if run.get("status") == "completed":
            if run.get("conclusion") != "success":
                raise ValueError("Source Checks did not succeed")
            return
        if index + 1 < polls:
            sleep(10)
    raise TimeoutError("Source Checks did not complete; no tag was created")


def discover(run_id, repository, root=ROOT, *, sha=None, attempt=None):
    run = api(f"repos/{repository}/actions/runs/{run_id}")
    require_identity(run, sha, attempt)
    # Read only this attempt, so a previous successful attempt cannot mask a failure.
    pages = api(f"repos/{repository}/actions/runs/{run_id}/attempts/{run['run_attempt']}/jobs?per_page=100")
    jobs = pages["jobs"]
    if pages["total_count"] != len(jobs):
        raise ValueError("Unexpected number of CI jobs; refusing a partial check")
    validate_ci(run, jobs, repository)
    return discover_candidate(run["head_sha"], repository, root)


def push_tag(tag, sha, root=ROOT):
    validate_tag(tag)
    if not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise ValueError("Invalid tag target SHA")
    ref = "refs/tags/" + tag
    remote = dict(line.split()[::-1] for line in git("ls-remote", "--tags", "origin", ref, ref + "^{}", root=root).decode().splitlines())
    if remote:
        if remote.get(ref + "^{}", remote.get(ref)) != sha:
            raise ValueError("Refusing to move existing release tag " + tag)
        return "already-present"
    local = subprocess.run(["git", "-C", str(root), "rev-parse", "--verify", "--quiet", ref + "^{commit}"], capture_output=True, text=True)
    if local.returncode == 0:
        if local.stdout.strip() != sha:
            raise ValueError("Local tag points to a different commit")
    else:
        git("tag", "-a", tag, sha, "-m", "Release " + tag, root=root)
    git("push", "origin", ref, root=root)
    return "created"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    discover_cmd = commands.add_parser("discover")
    discover_cmd.add_argument("--run-id", type=int, required=True)
    discover_cmd.add_argument("--wait", action="store_true")
    discover_cmd.add_argument("--sha")
    discover_cmd.add_argument("--attempt", type=int)
    tag_cmd = commands.add_parser("push")
    tag_cmd.add_argument("--tag", required=True)
    tag_cmd.add_argument("--sha", required=True)
    args = parser.parse_args()
    if args.command == "push":
        print(push_tag(args.tag, args.sha))
        return
    repository = os.environ["GITHUB_REPOSITORY"]
    if args.wait:
        if not args.sha or not args.attempt:
            parser.error("--wait requires --sha and --attempt")
        wait_for_ci(args.run_id, repository, args.sha, args.attempt)
    candidate = discover(args.run_id, repository, sha=args.sha, attempt=args.attempt)
    with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as stream:
        stream.write("eligible=" + ("true" if candidate else "false") + "\n")
        for key, value in (candidate or {}).items():
            stream.write(f"{key}={value}\n")
    message = (f"Validated version PR #{candidate['pr']}: {candidate['tag']} at {candidate['sha']}."
               if candidate else "No merged release-App version PR at this CI commit; no tag will be created.")
    print(message)
    with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as stream:
        stream.write(message + "\n")


if __name__ == "__main__":
    main()
