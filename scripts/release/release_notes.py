"""Freeze release descriptions before signing; render previews and accepted releases."""
import argparse
import json
from pathlib import Path
import re
import subprocess
from urllib.parse import quote
from scripts._lib.repository import ROOT
from scripts._lib.checksums import sha256
from scripts._lib.versioning import version
from scripts.release.note_entries import fragments, git, parse_fragment


TAG = re.compile(r"v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)")
GROUPS = [("breaking", "Breaking changes", "不兼容变化"), ("security", "Security", "安全修复"),
          ("feature", "Features", "新功能"), ("fix", "Fixes", "问题修复"), ("performance", "Performance", "性能改进")]


def resolve(ref, root):
    sha = git("rev-parse", "--verify", "--end-of-options", ref + "^{commit}", root=root).strip()
    if not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise ValueError("Invalid release source revision")
    return sha


def previous_tag(tag, revision, root=ROOT):
    target = tuple(map(int, TAG.fullmatch(tag).groups()))
    candidates = []
    for name in git("tag", "--merged", revision, root=root).splitlines():
        match = TAG.fullmatch(name)
        if match and tuple(map(int, match.groups())) <= target and resolve(name, root) != revision:
            candidates.append((tuple(map(int, match.groups())), name))
    return max(candidates)[1] if candidates else None


def collect(tag, revision="HEAD", *, base=None, root=ROOT):
    if not TAG.fullmatch(tag):
        raise ValueError("Expected vMAJOR.MINOR.PATCH")
    revision = resolve(revision, root)
    previous = previous_tag(tag, revision, root)
    start = resolve(base or previous, root) if base or previous else None
    if start:
        subprocess.run(["git", "merge-base", "--is-ancestor", start, revision], cwd=root, check=True)
    commit_range = start + ".." + revision if start else revision
    command = ["node", str(ROOT / "node_modules/git-cliff/lib/cli/cli.js"), "--config", str(ROOT / "cliff.toml"),
               "--repository", str(root), "--offline", "--no-exec", "--tag", tag, "--ignore-tags", ".*", commit_range]
    result = subprocess.run(command, cwd=root, text=True, encoding="utf-8", capture_output=True, check=True, timeout=90)
    return dict(schema=1, tag=tag, version=tag[1:], source_revision=revision, base_revision=start,
                previous_tag=previous if base is None else None, entries=fragments(start, revision, root=root),
                changelog=result.stdout.strip())


def validate(content, revision, number):
    if (content.get("schema") != 1 or content.get("source_revision") != revision
            or content.get("version") != number or content.get("tag") != "v" + number
            or not isinstance(content.get("changelog"), str) or not isinstance(content.get("entries"), list)):
        raise ValueError("Release descriptions do not match the accepted source/version")
    for entry in content["entries"]:
        parse_fragment(json_to_toml(entry), entry["path"])
    return content


def json_to_toml(entry):
    return "\n".join(key + " = " + json.dumps(entry[key], ensure_ascii=False) for key in ("kind", "en", "zh_CN"))


def bullets(content, locale):
    lines = []
    for kind, english, chinese in GROUPS:
        items = [entry[locale] for entry in content["entries"] if entry["kind"] == kind]
        if items:
            lines += ["### " + (english if locale == "en" else chinese), ""]
            for item in items:
                lines.append("- " + item.replace("\n", "\n  "))
            lines.append("")
    return "\n".join(lines).strip() or ("See the technical changelog for this release." if locale == "en" else "本版具体变化见技术变更列表。")


def update_summary(content):
    return "Wukong " + content["version"] + "\n\n" + bullets(content, "en") + "\n\n简体中文\n\n" + bullets(content, "zh_CN")


def render(content, *, directory=None):
    tag = content["tag"]
    revision = content["source_revision"]
    repository = "https://github.com/mark0-cn/Wukong"
    downloads, image = [], None
    if directory is not None:
        manifest = json.loads((directory / "release.json").read_text(encoding="utf-8"))
        updater = json.loads((directory / "latest.json").read_text(encoding="utf-8"))
        report = json.loads((directory / "validation-summary.json").read_text(encoding="utf-8"))
        validate(content, manifest["source_revision"], manifest["version"])
        if (not report.get("passed") or report.get("source_revision") != revision
                or report.get("image") != manifest["image"] or updater.get("source_revision") != revision
                or updater.get("version") != content["version"] or updater.get("image") != manifest["image"]
                or updater.get("notes") != update_summary(content)
                or updater.get("release_content_sha256") != sha256(directory / "release-content.json")):
            raise ValueError("Release descriptions or acceptance differ from the signed updater metadata")
        image = manifest["image"]
        for platform, label in (("windows-x86_64", "Windows x64"), ("linux-x86_64", "Linux x64")):
            asset = updater["platforms"][platform]
            name = asset["url"].rsplit("/", 1)[-1]
            expected_url = repository + "/releases/download/" + tag + "/" + quote(name)
            if (Path(name).name != name or asset["url"] != expected_url
                    or (directory / name).stat().st_size != asset["size"] or sha256(directory / name) != asset["sha256"]):
                raise ValueError("Installer link/hash/size differs from the accepted asset")
            downloads.append(f"| {label} | [{name}]({expected_url}) | {asset['size'] / 1024 / 1024:.1f} MiB |")
    download_table = "| Platform / 平台 | Installer / 安装程序 | Size / 大小 |\n| --- | --- | --- |\n" + "\n".join(downloads)
    english = [f"# Wukong {tag}", "", "## Highlights", "", bullets(content, "en"), "", "## Download and update", ""]
    chinese = ["## 本版变化", "", bullets(content, "zh_CN"), "", "## 下载与更新", ""]
    if downloads:
        english += [download_table, "", "New installation: download the installer for your platform. Existing users: open **Updates & environments** in Wukong. Prepare the matching tool environment when needed; normal public image pulls need no Docker Hub account.", ""]
        chinese += ["安装程序及大小见上方下载表。首次安装请选择对应平台；已有用户可在 Wukong 的**更新与环境**中更新，按需准备匹配的工具环境。正常拉取公开镜像无需 Docker Hub 账号。", ""]
        english += ["## Validation and limits", "", "Native installer smoke checks and Linux tool/container/debugger acceptance passed for these assets. Hosted runners do not cover full Windows Docker Desktop end-to-end installation. Windows installers currently have no Authenticode signature.", ""]
        chinese += ["## 验证与限制", "", "这些产物已通过原生安装程序冒烟检查和 Linux 工具、容器、调试器验收。托管 runner 未覆盖真实 Windows Docker Desktop 的完整安装流程。Windows 安装程序目前没有 Authenticode 签名。", ""]
    else:
        english += ["**Preview only.** Installers, image identity and validation results are added only after acceptance; this preview publishes nothing.", ""]
        chinese += ["**仅为预览。** 安装程序、镜像标识和验证结果仅在验收后补充，此预览不会发布任何内容。", ""]
    english += ["<details>", "<summary>简体中文</summary>", "", *chinese, "</details>", "", '<a id="technical-changelog"></a>', "<details>", "<summary>Technical changelog / 技术变更列表</summary>", "", content["changelog"] or "No additional commit entries / 无额外提交条目。", "", "</details>", "", "<details>", "<summary>Technical details / 技术详情</summary>", "", f"Source / 源码：[`{revision[:8]}`]({repository}/commit/{revision})", ""]
    if content.get("base_revision"):
        english += [f"[Full diff / 完整差异]({repository}/compare/{content['base_revision']}...{revision})", ""]
    if image:
        english += [f"Tool image / 工具镜像：`{image}`", "", "Checksums / 校验和：[SHA256SUMS](" + repository + "/releases/download/" + tag + "/SHA256SUMS).", "", "Updater metadata/signatures and acceptance reports are listed under Assets. / 更新元数据、签名与验收报告位于 Assets。", ""]
    return "\n".join([*english, "</details>", ""])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag")
    parser.add_argument("--revision", default="HEAD")
    parser.add_argument("--base")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--content", type=Path, help="Read a frozen content snapshot instead of Git history")
    parser.add_argument("--directory", type=Path, help="Accepted release assets (never pass for a preview)")
    args = parser.parse_args()
    content = json.loads(args.content.read_text(encoding="utf-8")) if args.content else collect(args.tag or "v" + version(), args.revision, base=args.base)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(render(content, directory=args.directory), encoding="utf-8")
    if not args.content:
        args.output.with_suffix(".json").write_text(json.dumps(content, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
