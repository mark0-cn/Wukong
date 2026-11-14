"""Fixed adapter executed only by the existing contained Python resource."""
import json
import os
from pathlib import Path
import subprocess
import sys

with open(sys.argv[1], encoding="utf-8") as source:
    configuration = json.load(source)

state = Path.cwd()
target = str((state / sys.argv[2]).resolve()) if len(sys.argv) == 3 else None
directory = state / configuration["cwd"]
directory.mkdir(parents=True, exist_ok=True)
environment = os.environ.copy()
environment.update(configuration["env"])
environment.update(WUKONG_STATE_DIR=str(state), WUKONG_JOB_DIR=str(directory))
if target is not None:
    environment["TARGET"] = target
shell = str(Path(os.environ["SystemRoot"]) / "System32" / "cmd.exe")
environment["COMSPEC"] = shell
# Keep cmd's command text intact. This is Python's Windows shell invocation
# form with fixed /d (disable AutoRun) and /s (outer quote handling) switches.
# subprocess owns process creation/handle inheritance; the native Job owns all
# descendants, deadlines and bounded output. Do not create a second supervisor.
prefix = subprocess.list2cmdline([shell, "/d", "/s", "/c"])
result = subprocess.run(
    prefix + ' "' + configuration["command"] + '"',
    executable=shell,
    cwd=directory,
    env=environment,
    creationflags=subprocess.CREATE_NO_WINDOW,
)
raise SystemExit(result.returncode)
