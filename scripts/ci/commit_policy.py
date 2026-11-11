"""Use commitlint for PR titles/commits and validate reviewed release summaries."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
from scripts._lib.repository import ROOT
from scripts.release.note_entries import fragments, pr_note


def git(*args):
    return subprocess.check_output(["git", *args], cwd=ROOT, text=True, encoding="utf-8").strip()


def lint(message):
    subprocess.run(["node", str(ROOT / "node_modules/@commitlint/cli/lib/cli.js"),
                    "--config", str(ROOT / "commitlint.config.mjs"), "--verbose"],
                   input=message, cwd=ROOT, text=True, encoding="utf-8", check=True)


def validate(pr):
    base, head = pr["base"]["sha"], pr["head"]["sha"]
    if not all(re.fullmatch(r"[0-9a-f]{40}", sha) for sha in (base, head)):
        raise ValueError("Invalid PR commit identity")
    lint(pr["title"])
    # Real merge commits are skipped by topology, never by a forgeable message.
    for sha in git("rev-list", "--reverse", "--no-merges", base + ".." + head).splitlines():
        print("Checking commit " + sha, flush=True)
        lint(git("show", "-s", "--format=%B", sha))
    start = git("merge-base", base, head)
    return pr_note(pr, fragments(start, head))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--event", type=Path, default=os.environ.get("GITHUB_EVENT_PATH"))
    args = parser.parse_args()
    if args.event is None:
        parser.error("--event or GITHUB_EVENT_PATH is required")
    pr = json.loads(Path(args.event).read_text(encoding="utf-8"))["pull_request"]
    print(json.dumps(validate(pr), ensure_ascii=False))


if __name__ == "__main__":
    main()
