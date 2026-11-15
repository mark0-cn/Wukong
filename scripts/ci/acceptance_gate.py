"""Require selected hosted acceptance jobs; never accept an unexpected skip."""
import json
import os


def checks_passed(needs, suite="selected", apt=False, uninstall=False):
    if suite not in ("selected", "all", "apt", "uninstall", "windows-worker"):
        return False
    selected = dict(apt=apt or suite in ("all", "apt"),
                    uninstall=uninstall or suite in ("all", "uninstall"),
                    **{"windows-worker": suite in ("all", "windows-worker")})
    return any(selected.values()) and all(
        needs.get(name, {}).get("result") == ("success" if enabled else "skipped")
        for name, enabled in selected.items())


if __name__ == "__main__":
    results = json.loads(os.environ["NEEDS_JSON"])
    print(json.dumps(results, indent=2))
    passed = checks_passed(results, os.environ["SUITE"], os.environ["APT"] == "true",
                           os.environ["UNINSTALL"] == "true")
    with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as stream:
        stream.write("Hosted acceptance: " + ("passed" if passed else "failed or incomplete") + "\n")
    raise SystemExit(0 if passed else 1)
