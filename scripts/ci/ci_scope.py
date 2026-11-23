"""Conservative PR/master check selection and a fail-closed CI result gate."""

from fnmatch import fnmatchcase
import json
import os
from pathlib import Path
import re
import subprocess
import tomllib
from scripts.ci import ci_reuse


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
        if ci_reuse.lightweight_path(path):
            continue
        if path.startswith("ui/"):
            result["frontend"] = True
            result["linux"] = True
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
    reuse = outputs.get("reuse")
    if reuse not in ("true", "false"):
        return False
    if reuse == "true" and (
        not outputs.get("reuse_source_run", "").isdigit() or not outputs.get("reuse_reason")
    ):
        return False
    selected = ci_reuse.selected_jobs(outputs)
    return all(
        needs.get(name, {}).get("result")
        == ("skipped" if reuse == "true" else "success" if enabled else "skipped")
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
    force_full = (
        clients
        or os.environ.get("WINDOWS_WORKER_ACCEPTANCE") == "true"
        or os.environ.get("FORCE_FULL_CI") == "true"
    )
    scope = select_scope(paths, event_name, head_branch, force_full=force_full)
    scope["release_clients"] = clients
    scope.update(
        reuse=False, reuse_source_run="", reuse_reason="Full checks without historical reuse"
    )
    if (
        event_name == "pull_request"
        and not force_full
        and not head_branch.startswith("codex/release-")
    ):
        current_scope = {name: str(scope[name]).lower() for name in ci_reuse.SCOPE_KEYS}
        try:
            current_merge = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
            current_tree = subprocess.check_output(
                ["git", "rev-parse", "HEAD^{tree}"], text=True
            ).strip()
            scope.update(
                ci_reuse.discover(
                    repository=os.environ["GITHUB_REPOSITORY"],
                    pr_number=int(pr["number"]),
                    head_branch=head_branch,
                    current_run=os.environ["GITHUB_RUN_ID"],
                    current_merge=current_merge,
                    current_tree=current_tree,
                    current_scope=current_scope,
                )
            )
        except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
            scope["reuse_reason"] = "Reuse discovery unavailable; running selected checks"
            print(
                f"::warning::Historical CI reuse unavailable ({type(error).__name__}); running selected checks"
            )
    with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as stream:
        for key, value in scope.items():
            stream.write(f"{key}={str(value).lower() if isinstance(value, bool) else value}\n")
    print(json.dumps(dict(scope=scope, changed_files=paths), indent=2))


if __name__ == "__main__":
    main()
