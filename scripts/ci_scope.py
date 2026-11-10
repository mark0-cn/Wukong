"""Conservative PR check selection and a fail-closed CI result gate."""
import json
import os
from pathlib import Path
import re
import subprocess
import tomllib


def select_scope(paths, event_name, head_branch=""):
    full = dict(rust=True, frontend=True, linux=True, reason="Full checks: non-PR or shared changes")
    if event_name != "pull_request" or head_branch.startswith("codex/release-") or not paths:
        return full
    result = dict(rust=False, frontend=False, linux=False, reason="PR checks selected from changed paths")
    for path in paths:
        if path == "README.md" or (path.startswith("docs/") and path.endswith(".md")):
            continue
        if path.startswith("ui/"):
            result["frontend"] = True
        elif path.startswith(("workers/runtime/", "fixtures/analysis/")):
            result["linux"] = True
        else:
            # Workflow, build, protocol, installer and unknown changes run everything.
            return full
    return result


def checks_passed(needs):
    if needs.get("changes", {}).get("result") != "success":
        return False
    outputs = needs["changes"].get("outputs", {})
    if any(outputs.get(name) not in ("true", "false") for name in ("rust", "frontend", "linux")):
        return False
    if needs.get("windows", {}).get("result") != "success":
        return False
    expected_linux = "success" if outputs["linux"] == "true" else "skipped"
    if needs.get("linux", {}).get("result") != expected_linux:
        return False
    # Older callers omit these jobs; new CI explicitly gates every selected prebuild.
    if "release_clients" in outputs or "windows-client" in needs or "linux-client" in needs:
        if outputs.get("release_clients") not in ("true", "false"):
            return False
        expected = "success" if outputs["release_clients"] == "true" else "skipped"
        return all(needs.get(name, {}).get("result") == expected for name in ("windows-client", "linux-client"))
    return True


def prebuild_clients(event_name, ref, requested, previous, current):
    if ref != "refs/heads/master":
        return False
    if event_name == "workflow_dispatch":
        return requested
    return event_name == "push" and tuple(map(int, current.split("."))) > tuple(map(int, previous.split(".")))


def main():
    event_name = os.environ["GITHUB_EVENT_NAME"]
    event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text(encoding="utf-8"))
    paths, head_branch = [], ""
    if event_name == "pull_request":
        pr = event["pull_request"]
        base, head = pr["base"]["sha"], pr["head"]["sha"]
        if not all(re.fullmatch(r"[0-9a-f]{40}", sha) for sha in (base, head)):
            raise ValueError("Invalid PR commit SHA")
        # --no-renames includes both old and new paths; a renamed shared file
        # must not evade full checks by moving into a selectively checked path.
        raw = subprocess.check_output(["git", "diff", "--name-only", "--no-renames", "-z", base + "..." + head])
        paths = [p.decode("utf-8", errors="replace") for p in raw.split(b"\0") if p]
        head_branch = pr["head"]["ref"]
    scope = select_scope(paths, event_name, head_branch)
    previous = current = "0.0.0"
    if event_name == "push" and os.environ.get("GITHUB_REF") == "refs/heads/master":
        previous = tomllib.loads(subprocess.check_output(["git", "show", "HEAD^1:Cargo.toml"]).decode())["package"]["version"]
        current = tomllib.loads(Path("Cargo.toml").read_text(encoding="utf-8"))["package"]["version"]
    scope["release_clients"] = prebuild_clients(event_name, os.environ.get("GITHUB_REF", ""),
                                                 os.environ.get("BUILD_RELEASE_CLIENTS") == "true", previous, current)
    with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as stream:
        for key, value in scope.items():
            stream.write(f"{key}={str(value).lower() if isinstance(value, bool) else value}\n")
    print(json.dumps(dict(scope=scope, changed_files=paths), indent=2))


if __name__ == "__main__":
    main()
