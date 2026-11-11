"""Read immutable, bilingual release fragments from the selected Git tree."""
import re
import subprocess
import tomllib
from scripts._lib.repository import ROOT


KINDS = ("feature", "fix", "performance", "security", "breaking")
FRAGMENT = re.compile(r"\.changes/[a-z0-9][a-z0-9-]{0,79}\.toml")


def git(*args, root=ROOT):
    return subprocess.check_output(["git", *args], cwd=root).decode("utf-8")


def parse_fragment(text, path):
    if not FRAGMENT.fullmatch(path):
        raise ValueError("Invalid release fragment path: " + path)
    data = tomllib.loads(text)
    if set(data) != {"kind", "en", "zh_CN"} or data["kind"] not in KINDS:
        raise ValueError(path + ": expected kind (feature/fix/performance/security/breaking), en and zh_CN")
    for key in ("en", "zh_CN"):
        value = data[key]
        if (not isinstance(value, str) or not value.strip() or len(value) > 2000
                or re.search(r"\b(?:TODO|TBD)\b|<!--", value, re.I)):
            raise ValueError(path + ": complete " + key + " (at most 2000 characters)")
    if not re.search(r"[a-zA-Z]", data["en"]) or not re.search(r"[\u3400-\u9fff]", data["zh_CN"]):
        raise ValueError(path + ": provide English and Chinese descriptions; review semantic equivalence manually")
    return dict(path=path, **data)


def fragments(base, head, *, root=ROOT):
    if base:
        raw = git("diff", "--name-status", "--no-renames", "-z", base, head, "--", ".changes", root=root).split("\0")
        changes = list(zip(raw[0:-1:2], raw[1:-1:2]))
    else:
        changes = [("A", p) for p in git("ls-tree", "-r", "--name-only", "-z", head, "--", ".changes", root=root).split("\0") if p]
    result = []
    for status, path in changes:
        if status != "A":
            raise ValueError("Release fragments are append-only; add a correction instead of editing/deleting " + path)
        result.append(parse_fragment(git("show", head + ":" + path, root=root), path))
    return sorted(result, key=lambda entry: entry["path"])


def pr_note(pr, entries):
    if entries:
        if "<!-- release-note:none:" in (pr.get("body") or ""):
            raise ValueError("Remove the internal-only marker when adding public release fragments")
        return dict(internal=False, fragments=[entry["path"] for entry in entries])
    title = pr.get("title", "")
    if (re.fullmatch(r"release: prepare v(?:0|[1-9][0-9]*)(?:\.(?:0|[1-9][0-9]*)){2}", title)
            and pr.get("head", {}).get("ref") == "codex/release-" + title.removeprefix("release: prepare ")):
        return dict(internal=True, reason="Version preparation only")
    if (pr.get("user", {}).get("login") == "dependabot[bot]"
            and re.match(r"^(?:build|chore)\(deps(?:-dev)?\): ", title)):
        return dict(internal=True, reason="Dependency details appear in the generated technical changelog")
    reasons = re.findall(r"<!-- release-note:none: ([^\r\n<>]+) -->", pr.get("body") or "")
    if len(reasons) != 1 or len(reasons[0].strip()) < 20:
        raise ValueError("Add a bilingual .changes/<slug>.toml fragment, or an English <!-- release-note:none: reason --> marker (at least 20 characters)")
    return dict(internal=True, reason=reasons[0].strip())
