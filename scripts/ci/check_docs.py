"""Offline documentation pairing/link checks and explicit review fingerprints.

Fingerprints detect unreviewed edits; they do not establish translation accuracy.
"""
import argparse
import hashlib
import html
import json
import re
import subprocess
from pathlib import Path
from urllib.parse import unquote, urlsplit

ALIASES = {
    "README.en.md": "README.md",
    **{f"docs/en/user/{name}.md": f"docs/user/{name}.md"
       for name in ("usage", "online-install", "languages")},
}
RECORD = "docs/translations.json"


def prose(text):
    return re.sub(r"^(`{3,}|~{3,})[^\n]*\n.*?^\1\s*$", "", text, flags=re.M | re.S)


def headings(text):
    return re.findall(r"^(#{1,6})\s+(.+?)\s*#*\s*$", prose(text), re.M)


def slug(text):
    text = re.sub(r"<[^>]*>", "", html.unescape(text)).lower()
    text = re.sub(r"[^\w\- ]", "", text, flags=re.UNICODE)
    return text.replace(" ", "-")


def anchors(text):
    result = set(re.findall(r'<a\s+(?:id|name)=[\'"]([^\'"]+)', prose(text)))
    counts = {}
    for _, title in headings(text):
        key = slug(title)
        count = counts.get(key, 0)
        counts[key] = count + 1
        result.add(key if not count else f"{key}-{count}")
    return result


def links(text):
    text = re.sub(r"`[^`\n]+`", "", prose(text))
    return (re.findall(r"\]\(([^\s)]+)(?:\s+[^)]*)?\)", text)
            + re.findall(r'(?:href|src)=[\'"]([^\'"]+)', text))


def chinese_path(path):
    return path[:-3] + ".zh-CN.md"


def text_hash(text):
    return hashlib.sha256(text.replace("\r\n", "\n").encode("utf-8")).hexdigest()


def check(root, files, reviewed=None):
    docs = sorted({p for p in files if p.endswith(".md") and not p.startswith("prompts/")})
    texts, errors, pairs = {}, [], {}
    for path in docs:
        try:
            texts[path] = (root / path).read_text(encoding="utf-8")
        except (OSError, UnicodeError) as error:
            errors.append(f"{path}: cannot read UTF-8: {error}")
    for path, text in texts.items():
        if path in ALIASES:
            expected = ALIASES[path]
            if not any((root / path).parent.joinpath(urlsplit(link).path).resolve() == (root / expected).resolve()
                       for link in links(text) if not urlsplit(link).scheme):
                errors.append(f"{path}: missing canonical compatibility link")
            continue
        english = path.replace(".zh-CN.md", ".md")
        chinese = chinese_path(english)
        counterpart = english if path == chinese else chinese
        if counterpart not in texts:
            errors.append(f"{path}: missing counterpart {counterpart}")
            continue
        expected_link = f"]({Path(counterpart).name})"
        if expected_link not in text[:1200] and f'href="{Path(counterpart).name}"' not in text[:1200]:
            errors.append(f"{path}: missing reciprocal language link near the top")
        if path == english:
            if [level for level, _ in headings(text)] != [level for level, _ in headings(texts[chinese])]:
                errors.append(f"{path}: English/Chinese section structure differs")
            pairs[english] = {"en": text_hash(text), "zh-CN": text_hash(texts[chinese])}
    for path, text in texts.items():
        for link in links(text):
            link = html.unescape(link).strip("<>")
            parsed = urlsplit(link)
            if parsed.scheme or parsed.netloc:
                continue
            target = ((root / path).parent / unquote(parsed.path)).resolve() if parsed.path else (root / path).resolve()
            if not target.is_relative_to(root.resolve()):
                errors.append(f"{path}: link leaves repository: {link}")
                continue
            if not target.exists():
                errors.append(f"{path}: missing link target: {link}")
                continue
            target_name = target.relative_to(root.resolve()).as_posix()
            if target_name in texts and parsed.fragment and unquote(parsed.fragment) not in anchors(texts[target_name]):
                errors.append(f"{path}: missing anchor: {link}")
            # Only the language switch should cross languages.
            if path not in ALIASES and target_name in texts and target_name not in ALIASES:
                own_base = path.replace(".zh-CN.md", ".md")
                target_base = target_name.replace(".zh-CN.md", ".md")
                if own_base != target_base and path.endswith('.zh-CN.md') != target_name.endswith('.zh-CN.md'):
                    errors.append(f"{path}: cross-language document link: {link}")
    if reviewed is not None:
        for path in sorted(pairs.keys() | reviewed.keys()):
            if pairs.get(path) != reviewed.get(path):
                errors.append(f"{path}: pair changed since review; review both languages, then --record")
    return errors, pairs


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--record", action="store_true", help="Record manually reviewed pairs after structural checks")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    raw = subprocess.check_output(["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"], cwd=root)
    files = [p.decode("utf-8") for p in raw.split(b"\0") if p]
    reviewed = None
    if not args.record:
        try:
            reviewed = json.loads((root / RECORD).read_text(encoding="utf-8"))["pairs"]
        except (OSError, ValueError, KeyError) as error:
            parser.exit(1, f"Cannot read review record: {error}\n")
    errors, pairs = check(root, files, reviewed)
    if errors:
        parser.exit(1, "\n".join(errors) + "\n")
    if args.record:
        (root / RECORD).write_text(json.dumps({"schema_version": 1, "pairs": pairs}, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        print(f"Recorded {len(pairs)} pairs; this records your review, not automatic semantic validation.")
    else:
        print(f"Documentation checks passed: {len(pairs)} pairs, local links/anchors, review fingerprints.")


if __name__ == "__main__":
    main()
