"""Remove caches belonging only to closed PRs; preserve master and all live PR refs."""
import argparse
import os
import re
import subprocess
from scripts._lib.github import api


def candidates(caches, closed, opened, repository):
    open_numbers = {p["number"] for p in opened}
    open_heads = {p["head"]["ref"] for p in opened if (p["head"].get("repo") or {}).get("full_name") == repository}
    closed_numbers = {p["number"] for p in closed if p.get("state") == "closed"}
    bot_heads = {p["head"]["ref"] for p in closed if p.get("state") == "closed"
                 and p.get("user", {}).get("login") == "dependabot[bot]"
                 and (p["head"].get("repo") or {}).get("full_name") == repository
                 and p["head"]["ref"].startswith("dependabot/")}
    selected = []
    for cache in caches:
        ref = cache["ref"]
        pr = re.fullmatch(r"refs/pull/([1-9][0-9]*)/merge", ref)
        if pr and int(pr[1]) in closed_numbers - open_numbers:
            selected.append(cache)
        elif ref.startswith("refs/heads/dependabot/") and ref.removeprefix("refs/heads/") in bot_heads - open_heads:
            selected.append(cache)
    return selected


def all_caches(repository):
    result = []
    for page in range(1, 101):
        data = api(f"repos/{repository}/actions/caches?per_page=100&page={page}")["actions_caches"]
        result.extend(data)
        if len(data) < 100:
            return result
    raise ValueError("Too many caches; refuse an incomplete inventory")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--apply", action="store_true")
    args = parser.parse_args()
    repository = os.environ["GITHUB_REPOSITORY"]
    caches = all_caches(repository)
    # Read both states immediately before selecting IDs, never delete by broad key prefix.
    closed = api(f"repos/{repository}/pulls?state=closed&per_page=100", paginate=True)
    opened = api(f"repos/{repository}/pulls?state=open&per_page=100", paginate=True)
    selected = candidates(caches, closed, opened, repository)
    total = sum(c["size_in_bytes"] for c in selected)
    lines = [f"## {'Removed' if args.apply else 'Would remove'} closed-PR caches", "",
             f"{len(selected)} caches, {total / 1024 ** 3:.2f} GiB. Master, tags and live PR caches are retained.", ""]
    for cache in selected:
        if args.apply:
            subprocess.run(["gh", "api", "--method", "DELETE", f"repos/{repository}/actions/caches/{cache['id']}"], check=True)
        lines.append(f"- ID {cache['id']}: {cache['ref']} ({cache['size_in_bytes'] / 1024 ** 2:.1f} MiB)")
    output = "\n".join(lines) + "\n"
    print(output)
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as stream:
            stream.write(output)


if __name__ == "__main__":
    main()
