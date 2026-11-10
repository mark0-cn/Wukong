"""Authenticated GitHub CLI JSON requests shared by automation."""
import json
import subprocess


def api(path, paginate=False):
    args = ["gh", "api"]
    if paginate:
        args += ["--paginate", "--slurp"]
    data = json.loads(subprocess.check_output([*args, path]))
    return [item for page in data for item in page] if paginate else data


