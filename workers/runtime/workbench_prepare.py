"""Build a declarative launch description without executing challenge code."""
import json
import os
from pathlib import Path
import shutil
import sys
from worker_common import sha256_file as digest


def inside(root, name):
    path=(root/name).resolve()
    if not path.is_relative_to(root) or not path.exists(): raise ValueError("missing or escaping challenge path: "+name)
    return path


def owned_mode(path, mode):
    # Docker copies may retain a host UID. The trusted supervisor has CHOWN,
    # but deliberately lacks FOWNER, so take ownership before changing mode.
    if os.geteuid() == 0 and path.stat().st_uid != 0:
        os.chown(path, 0, 0)
    path.chmod(mode)


def prepare(root):
    root=root.resolve()
    manifest=json.loads((root/"launch-request.json").read_text())
    target=manifest["target"].get("binary")
    runtime=manifest["runtime"]
    spec={"schema_version":1,"challenge_id":manifest["id"],"base":"files/challenge",
          "target":target,"args":manifest["launch"]["argv"],"cwd":manifest["launch"]["cwd"],
          "input_mode":manifest["launch"]["input"],"env":manifest["launch"]["env"],
          "timeout_secs":manifest["launch"]["timeout_seconds"],"files":{},"runtime_confirmed":False,
          "service":manifest["network"],"loader":None,"library_path":None}
    if target:
        binary=inside(root,target); spec["files"][target]=digest(binary)
        owned_mode(binary,0o555)
    if runtime["kind"]=="loader" or runtime.get("libc") or runtime.get("loader"):
        if not target or not runtime.get("libc") or not runtime.get("loader"):
            raise ValueError("loader runtime requires target, libc and loader")
        library=root/"prepared"/"libraries"; library.mkdir(parents=True,exist_ok=True)
        for source,name in [(runtime["libc"],"libc.so.6"),(runtime["loader"],"ld.so")]:
            path=inside(root,source); output=library/name
            if output.exists(): owned_mode(output,0o644)
            shutil.copyfile(path,output); owned_mode(output,0o555)
            spec["files"][str(output.relative_to(root))]=digest(output)
        spec.update(loader="prepared/libraries/ld.so",library_path="prepared/libraries")
    inside(root,spec["cwd"])
    count,total=0,0
    for path in inside(root,spec['cwd']).rglob('*'):
        if path.is_symlink(): raise ValueError('launch cwd cannot contain symlinks')
        if path.is_file() and path not in {root/'launch.json',root/'launch-request.json'}:
            count+=1;total+=path.stat().st_size
            if count>1024 or total>32*1048576: raise ValueError('launch cwd exceeds 1024 files/32 MiB')
            spec['files'][path.relative_to(root).as_posix()]=digest(path)
    if spec["input_mode"]=="argv_file" and "@@" not in spec["args"]:
        raise ValueError("argv_file requires an @@ argument")
    # Original files and prepared copies are never mistaken for verified runtime identity.
    (root/"launch.json").write_text(json.dumps(spec,indent=2))
    return spec


if __name__=="__main__": print(json.dumps(prepare(Path(sys.argv[1]))))
