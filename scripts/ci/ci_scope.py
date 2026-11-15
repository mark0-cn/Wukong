"""Conservative PR/master check selection and a fail-closed CI result gate."""

from fnmatch import fnmatchcase
import json
import os
from pathlib import Path
import re
import subprocess
import tomllib
from scripts.release.note_entries import FRAGMENT


APT_PATHS = (
    "scripts/ci/install_apt.py",
    "tests/automation/test_install_apt.py",
    "tests/acceptance/check_ci_apt.py",
)
UNINSTALL_PATHS = (
    "src/application/maintenance/cleanup/*",
    "src/interfaces/cli/cleanup.rs",
    "src/interfaces/desktop/cleanup.rs",
    "src/interfaces/desktop/cleanup_review.rs",
    "src/adapters/files/*",
    "src/main.rs",
    "src/application/maintenance/updates/recovery.rs",
    "packaging/windows/*",
    "tauri*.json",
    "tests/acceptance/check_cleanup*",
    "tests/acceptance/review-window.ps1",
    "ui/src/features/maintenance/CleanupReview.tsx",
    "ui/src/main.tsx",
    "ui/src/shared/api/commands.ts",
    "locales/*/cleanup.ftl",
)
SCOPE_FLAGS = ("rust", "frontend", "linux", "apt", "uninstall")


def select_scope(paths, event_name, head_branch="", force_full=False):
    full = dict.fromkeys(SCOPE_FLAGS, True)
    full["reason"] = "Full checks: explicit request, release, or unavailable change range"
    if (
        force_full
        or event_name not in ("pull_request", "push")
        or head_branch.startswith("codex/release-")
        or not paths
    ):
        return full
    workflow = any(p.startswith((".github/workflows/", ".github/actions/")) for p in paths)
    acceptance = dict(
        apt=workflow or any(p in APT_PATHS for p in paths),
        uninstall=workflow
        or any(fnmatchcase(p, pattern) for p in paths for pattern in UNINSTALL_PATHS),
    )
    result = dict(
        rust=False,
        frontend=False,
        linux=False,
        **acceptance,
        reason="Checks selected from changed paths",
    )
    for path in paths:
        # Required commit-policy checks validate these data-only release notes.
        # Keep unknown files under .changes conservative, just like other paths.
        if FRAGMENT.fullmatch(path):
            continue
        if path in (
            "README.md",
            "README.en.md",
            "README.zh-CN.md",
            "AGENTS.md",
            "AGENTS.zh-CN.md",
            "docs/translations.json",
        ) or (path.startswith("docs/") and path.endswith(".md")):
            continue
        if path.startswith("ui/"):
            result["frontend"] = True
        elif path.startswith(("workers/runtime/", "workers/tests/", "fixtures/", "evals/")):
            result["linux"] = True
        else:
            # Workflow, build, protocol, installer and unknown changes run everything.
            return dict(full, **acceptance, reason="Full component checks: shared or unknown paths")
    return result


def checks_passed(needs):
    if needs.get("changes", {}).get("result") != "success":
        return False
    outputs = needs["changes"].get("outputs", {})
    if any(
        outputs.get(name) not in ("true", "false") for name in (*SCOPE_FLAGS, "release_clients")
    ):
        return False
    selected = dict(
        windows=any(outputs[k] == "true" for k in ("rust", "frontend", "linux")),
        linux=outputs["linux"] == "true",
        acceptance=any(outputs[k] == "true" for k in ("apt", "uninstall")),
        **{
            name: outputs["release_clients"] == "true"
            for name in ("windows-client", "linux-client", "linux-desktop-client")
        },
    )
    return all(
        needs.get(name, {}).get("result") == ("success" if enabled else "skipped")
        for name, enabled in selected.items()
    )


def changed_paths(base, head, *, pull_request=False):
    if not all(re.fullmatch(r"[0-9a-f]{40}", sha) and sha != "0" * 40 for sha in (base, head)):
        raise ValueError("Invalid change-range commit SHA")
    # Renames must expose both paths. Pushes compare the entire pushed range,
    # not only HEAD^1, which would miss earlier commits in a multi-commit push.
    revision = base + ("..." if pull_request else "..") + head
    raw = subprocess.check_output(["git", "diff", "--name-only", "--no-renames", "-z", revision])
    return [p.decode("utf-8", errors="replace") for p in raw.split(b"\0") if p]


def prebuild_clients(event_name, ref, requested, previous, current):
    if ref != "refs/heads/master":
        return False
    if event_name == "workflow_dispatch":
        return requested
    return event_name == "push" and tuple(map(int, current.split("."))) > tuple(
        map(int, previous.split("."))
    )


def main():
    event_name = os.environ["GITHUB_EVENT_NAME"]
    event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text(encoding="utf-8"))
    paths, head_branch = [], ""
    if event_name == "pull_request":
        pr = event["pull_request"]
        base, head = pr["base"]["sha"], pr["head"]["sha"]
        paths = changed_paths(base, head, pull_request=True)
        head_branch = pr["head"]["ref"]
    elif event_name == "push" and os.environ.get("GITHUB_REF") == "refs/heads/master":
        paths = changed_paths(event["before"], event["after"])
    previous = current = "0.0.0"
    if event_name == "push" and os.environ.get("GITHUB_REF") == "refs/heads/master":
        previous = tomllib.loads(
            subprocess.check_output(["git", "show", "HEAD^1:Cargo.toml"]).decode()
        )["package"]["version"]
        current = tomllib.loads(Path("Cargo.toml").read_text(encoding="utf-8"))["package"][
            "version"
        ]
    clients = prebuild_clients(
        event_name,
        os.environ.get("GITHUB_REF", ""),
        os.environ.get("BUILD_RELEASE_CLIENTS") == "true",
        previous,
        current,
    )
    scope = select_scope(
        paths,
        event_name,
        head_branch,
        force_full=clients or os.environ.get("WINDOWS_WORKER_ACCEPTANCE") == "true",
    )
    scope["release_clients"] = clients
    with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as stream:
        for key, value in scope.items():
            stream.write(f"{key}={str(value).lower() if isinstance(value, bool) else value}\n")
    print(json.dumps(dict(scope=scope, changed_files=paths), indent=2))


if __name__ == "__main__":
    main()
