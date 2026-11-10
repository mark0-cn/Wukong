"""Render an Actions summary from job results and the actual validation/package artifacts."""
import argparse
from datetime import datetime
import html
import json
import os
from pathlib import Path
import urllib.error
import urllib.request
from ci_scope import checks_passed


def cell(value):
    return html.escape(str(value)).replace("|", "&#124;").replace("\n", " ").replace("\r", " ").replace("`", "&#96;")


def api(path):
    headers = {"Accept": "application/vnd.github+json", "User-Agent": "re-agent-actions-summary"}
    if os.environ.get("GH_TOKEN"):
        headers["Authorization"] = "Bearer " + os.environ["GH_TOKEN"]
    # REST endpoints used here return JSON directly; do not forward tokens on redirects.
    class NoRedirect(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, *args, **kwargs):
            return None
    request = urllib.request.Request("https://api.github.com" + path, headers=headers)
    with urllib.request.build_opener(NoRedirect()).open(request, timeout=20) as response:
        data = response.read(4 * 1024 * 1024 + 1)
    if len(data) > 4 * 1024 * 1024:
        raise ValueError("Summary metadata exceeded 4 MiB")
    return json.loads(data)


def read(path):
    return json.loads(path.read_text(encoding="utf-8-sig")) if path.is_file() else None


def duration(name, jobs):
    selected = [j for j in jobs if j["name"] == name or j["name"].endswith(" / " + name) or j["name"].startswith(name + " / ")]
    if not selected or any(not j.get("started_at") or not j.get("completed_at") for j in selected):
        return "—"
    start = min(datetime.fromisoformat(j["started_at"].replace("Z", "+00:00")) for j in selected)
    end = max(datetime.fromisoformat(j["completed_at"].replace("Z", "+00:00")) for j in selected)
    minutes, seconds = divmod(max(0, int((end - start).total_seconds())), 60)
    return f"{minutes}m {seconds}s"


def render(kind, needs, context, directory, jobs=(), artifacts=(), warnings=()):
    failed = any(v["result"] in {"failure", "cancelled"} for v in needs.values())
    if kind == "release":
        passed = all(needs.get(n, {}).get("result") == "success" for n in
                     ("metadata", "checks", "windows-client", "linux-client", "image", "acceptance", "package"))
        if context.get("event") == "push":
            passed = passed and needs.get("publish", {}).get("result") == "success"
    else:
        passed = checks_passed(needs) if "changes" in needs else all(needs.get(n, {}).get("result") == "success" for n in ("windows", "linux"))
    state = "通过 / Passed" if passed and not failed else "未完成或失败 / Incomplete or failed"
    run_url = context["run_url"]
    lines = [f"## {kind.upper()} — {state}", "", f"- Ref: {cell(context['ref'])}",
             f"- Commit: `{cell(context['sha'])}`", f"- [本次运行与日志]({run_url})", "",
             "| 任务 | 结果 | 执行耗时 |", "| --- | --- | --- |"]
    for name, value in needs.items():
        lines.append(f"| {cell(name)} | {cell(value['result'])} | {duration(name, jobs)} |")
    lines += ["", "耗时来自本次尝试的 GitHub job 记录，不含排队；组合任务显示起止跨度。缺失时显示 —。", ""]
    if kind == "ci" and "changes" in needs:
        scope = needs["changes"].get("outputs", {})
        lines += ["### 检查范围", "", cell(scope.get("reason", "范围计算失败")), "",
                  "Windows 版本及安装器检查始终执行；未选择的重型检查按规则跳过。",
                  *[f"- {name}: {cell(scope.get(name, 'unknown'))}" for name in ("rust", "frontend", "linux")], ""]
        cache_hit = needs.get("windows", {}).get("outputs", {}).get("rust_cache_hit", "")
        cache_state = {"true": "精确命中", "false": "未精确命中（回退恢复或无缓存，详见 Rust cache 日志）"}.get(cache_hit, "未执行或无结果")
        lines += ["### Rust 缓存与步骤耗时", "", f"- Rust cache: {cache_state}", "",
                  "| Job | 步骤 | 结果 | 耗时 |", "| --- | --- | --- | --- |"]
        measured = {"Rust cache", "Post Rust cache", "Rust formatting", "Rust Clippy", "Rust tests",
                    "Frontend", "Version and release tooling tests", "Python runtime and release tests"}
        for job in jobs:
            for step in job.get("steps", []):
                if step["name"] in measured:
                    lines.append(f"| {cell(job['name'])} | {cell(step['name'])} | {cell(step.get('conclusion') or step.get('status', 'unknown'))} | {duration(step['name'], [step])} |")
        lines += ["", "缓存保存耗时显示在 Post Rust cache；未写入缓存时，该步骤可能仅执行清理或直接跳过。", ""]
    if kind == "release":
        manifest = read(directory / "release/release.json")
        validation = read(directory / "validation/validation-summary.json") or read(directory / "release/validation-summary.json")
        image = read(directory / "validation/image.json")
        metadata = needs.get("metadata", {}).get("outputs", {})
        digest = needs.get("image", {}).get("outputs", {}).get("digest")
        image_ref = manifest.get("image") if manifest else None
        if not image_ref and image:
            image_ref = image["repository"] + "@" + image["digest"]
        if not image_ref and digest and metadata.get("image"):
            image_ref = metadata["image"] + "@" + digest
        lines += ["### 产物与验收", "", f"- Version: {cell(manifest['version'] if manifest else metadata.get('version', '未生成'))}",
                  f"- Image: `{cell(image_ref or '未生成')}`"]
        if manifest:
            lines += [f"- Tool platform: {cell(manifest['tool_platform'])}",
                      f"- 镜像解包大小: {manifest['image_size_bytes'] / (1024 ** 3):.2f} GiB"]
            for archive in sorted((directory / "release").glob("*.zip")):
                lines.append(f"- {cell(archive.name)}: {archive.stat().st_size / (1024 ** 2):.2f} MiB")
        if validation:
            lines.append("- 容器验收: " + ("通过" if validation.get("passed") else "失败"))
            for name, result in validation.get("checks", {}).items():
                lines.append(f"  - {cell(name)}: {cell(result)}")
            if validation.get("error"):
                lines.append("- 验收错误: " + cell(validation["error"])[:1500])
        else:
            lines.append("- 尚无容器验收报告；请查看任务结果和日志。")
        lines.append("- Windows Docker Desktop 完整桌面安装不在托管 runner 验收范围内。")
        if context.get("event") == "workflow_dispatch":
            lines.append("- 本次为手动演练：不创建正式 Release，不更新版本标签或 latest。")
        elif needs.get("publish", {}).get("result") == "success":
            lines.append(f"- [已发布的不可变 Release](https://github.com/{context['repository']}/releases/tag/{context['ref_name']})")
        else:
            lines.append("- 正式发布未成功完成；失败时可能留有草稿或部分发布结果，请查看 publish 日志。")
    lines += ["", "### 下载", ""]
    useful = [a for a in artifacts if a["name"] in {"online-release", "image-validation", "windows-client"} and not a.get("expired")]
    for artifact in useful:
        url = f"https://github.com/{context['repository']}/actions/runs/{context['run_id']}/artifacts/{artifact['id']}"
        lines.append(f"- [{cell(artifact['name'])}]({url}) — {artifact['size_in_bytes'] / (1024 ** 2):.2f} MiB; expires {cell(artifact.get('expires_at', 'unknown'))}")
    if not useful:
        lines.append(f"- 暂无相关产物；查看[运行页面]({run_url})。")
    if warnings:
        lines += ["", "### 摘要元数据提示", "", *["- " + cell(w) for w in warnings]]
    return "\n".join(lines) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kind", choices=["ci", "release"], required=True)
    parser.add_argument("--directory", type=Path, default=Path("summary-artifacts"))
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    repository = os.environ["GITHUB_REPOSITORY"]
    run_id = os.environ["GITHUB_RUN_ID"]
    context = {"repository": repository, "run_id": run_id, "run_url": f"https://github.com/{repository}/actions/runs/{run_id}",
               "ref": os.environ.get("GITHUB_REF", ""), "ref_name": os.environ.get("GITHUB_REF_NAME", ""),
               "sha": os.environ.get("GITHUB_SHA", ""), "event": os.environ.get("GITHUB_EVENT_NAME", "")}
    jobs, artifacts, warnings = [], [], []
    endpoints = [("jobs", f"/repos/{repository}/actions/runs/{run_id}/attempts/{os.environ.get('GITHUB_RUN_ATTEMPT', '1')}/jobs?per_page=100", jobs),
                 ("artifacts", f"/repos/{repository}/actions/runs/{run_id}/artifacts?per_page=100", artifacts)]
    for field, path, values in endpoints:
        try:
            values.extend(api(path)[field])
        except (OSError, ValueError, KeyError) as error:
            # Job conclusions are supplied by needs, not inferred from this optional API metadata.
            warnings.append(f"Could not read {field}: {error}")
    output = render(args.kind, json.loads(os.environ["NEEDS_JSON"]), context, args.directory, jobs, artifacts, warnings)
    if args.output:
        args.output.write_text(output, encoding="utf-8")
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as stream:
            stream.write(output)
    print(output)


if __name__ == "__main__":
    main()
