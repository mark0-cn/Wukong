"""Preview current PR changes and cumulative release notes without publication."""
import json
import os
from pathlib import Path
import tomllib
from scripts._lib.repository import ROOT
from scripts.release.note_entries import git
from scripts.release.release_notes import collect, render, resolve


def preview(pr, directory, *, root=ROOT):
    head = resolve(pr["head"]["sha"], root)
    base = git("merge-base", pr["base"]["sha"], head, root=root).strip()
    number = tomllib.loads(git("show", head + ":Cargo.toml", root=root))["package"]["version"]
    tag = "v" + number
    incremental = collect(tag, head, base=base, root=root)
    cumulative = collect(tag, head, root=root)
    directory.mkdir(parents=True, exist_ok=True)
    for name, content in (("pr-changes", incremental), ("next-release", cumulative)):
        (directory / (name + ".json")).write_text(json.dumps(content, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        (directory / (name + ".md")).write_text(render(content), encoding="utf-8")
    is_version = pr["head"].get("ref", "").startswith("codex/release-")
    summary = ("## Release description preview\n\n"
               "Preview only; no tag, image or release is published. PR text requires English only.\n\n"
               f"Source: `{head}`. " + (f"Proposed version: `{tag}`.\n\n" if is_version else f"Source version: `{tag}`; the next version is chosen during version preparation.\n\n")
               + "<details>\n<summary>" + ("Cumulative version changes" if is_version else "Changes in this PR") + "</summary>\n\n"
               + render(cumulative if is_version else incremental) + "\n</details>\n\n"
               + "Both the PR-only and cumulative Markdown/JSON previews are in the release-description-preview artifact. "
                 "Review the Chinese translation and internal-only declarations manually.\n")
    # Preserve full output in artifacts without exceeding GitHub's summary limit.
    if len(summary.encode("utf-8")) > 200_000:
        summary = f"## Release description preview\n\nSource: `{head}`. Preview exceeds the inline limit; open the release-description-preview artifact. No publication occurred.\n"
    return summary


def main():
    event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text(encoding="utf-8"))
    summary = preview(event["pull_request"], ROOT / "target/release-description-preview")
    with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as stream:
        stream.write(summary)


if __name__ == "__main__":
    main()
