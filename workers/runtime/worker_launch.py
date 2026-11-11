"""One checked launch description for interaction, debugging and replay."""
import hashlib
import json
from pathlib import Path
import re
import os
import shutil
import subprocess
import tempfile
from worker_common import public_file, safe_env


from worker_common import sha256_file as digest
from worker_launch_spec import read_launch


def load(root,params):
    document = read_launch(root, params["launch_spec"])
    spec, base, member = document.spec, document.base, document.member
    if not spec.get("target"): raise ValueError("service-only challenge has no local target; use a network script")
    files = document.check_files()
    target = member(spec["target"])
    if params.get("target") and digest(public_file(root,params["target"]))!=digest(target):
        raise ValueError("requested target differs from LaunchSpec")
    if spec.get("loader") and spec["loader"] not in files: raise ValueError("loader identity missing")
    args=spec.get("args",[])
    if not isinstance(args,list) or len(args)>64 or any(not isinstance(a,str) or "\0" in a for a in args):
        raise ValueError("invalid launch arguments")
    mode=spec.get("input_mode")
    if mode not in {"stdin_pipe","argv_file","pty"}: raise ValueError("invalid launch input mode")
    if mode=="argv_file" and "@@" not in args: raise ValueError("argv_file needs @@")
    if params.get("args") is not None and params["args"]!=args: raise ValueError("arguments must be changed in LaunchSpec, not per-call")
    if params.get("env"): raise ValueError("environment must be changed in LaunchSpec, not per-call")
    env=safe_env()
    for key,value in spec.get("env",{}).items():
        # These could execute code inside a privileged exec-wrapper/interpreter.
        if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*",key) or key.startswith(("LD_","DYLD_","PYTHON","GDB","WUKONG_")) or key in {"PATH","ENV","BASH_ENV","SHELLOPTS","HOME","TMPDIR"}:
            raise ValueError("unsafe launch environment variable: "+key)
        if not isinstance(value,str) or "\0" in value: raise ValueError("invalid launch environment value")
        env[key]=value
    cwd=member(spec["cwd"])
    if not cwd.is_dir(): raise ValueError("launch cwd must be a directory")
    return spec,base,target,cwd,env


def patched_runtime(base,spec):
    """Public, root-owned immutable runtime. Stable paths keep trial hashes stable."""
    import fcntl
    parent=Path("/tmp/wukong-launches")
    parent.mkdir(mode=0o755,exist_ok=True)
    st=parent.lstat()
    if parent.is_symlink() or st.st_uid!=os.geteuid() or st.st_mode&0o022:
        raise ValueError("unsafe public launch cache")
    key=hashlib.sha256(json.dumps(spec["files"],sort_keys=True).encode()).hexdigest()
    destination=parent/key
    with (parent/(key+".lock")).open("a+b") as lock:
        fcntl.flock(lock,fcntl.LOCK_EX)
        if not destination.exists():
            temporary=Path(tempfile.mkdtemp(dir=parent))
            try:
                libraries=temporary/"libraries";libraries.mkdir()
                library=(base/spec["library_path"]).resolve()
                for item in library.iterdir():
                    if not item.is_file() or item.is_symlink(): raise ValueError("runtime library must be a regular file")
                    shutil.copyfile(item,libraries/item.name);(libraries/item.name).chmod(0o555)
                binary=temporary/"target";shutil.copyfile(base/spec["target"],binary);binary.chmod(0o755)
                loader=destination/"libraries"/Path(spec["loader"]).name
                subprocess.run(["patchelf","--set-interpreter",str(loader),"--set-rpath",str(destination/"libraries"),str(binary)],check=True,capture_output=True,timeout=30)
                binary.chmod(0o555)
                record={p.relative_to(temporary).as_posix():digest(p) for p in temporary.rglob("*") if p.is_file()}
                (temporary/"identity.json").write_text(json.dumps(record));(temporary/"identity.json").chmod(0o444)
                libraries.chmod(0o555);temporary.chmod(0o555)
                temporary.rename(destination)
            except Exception:
                shutil.rmtree(temporary);raise
        if destination.is_symlink() or destination.stat().st_uid!=os.geteuid() or destination.stat().st_mode&0o022:
            raise ValueError("unsafe cached runtime")
        for name,expected in json.loads((destination/"identity.json").read_text()).items():
            if digest(destination/name)!=expected: raise ValueError("cached runtime was modified")
    return destination/"target"


def resolve(root,params,isolated=False,input_path=None):
    root=Path(root).resolve()
    spec,base,target,cwd,env=load(root,params)
    loader=(base/spec["loader"]).resolve() if spec.get("loader") else None
    library=base/spec["library_path"] if loader else None
    if library:
        library=library.resolve()
        if not library.is_relative_to(base): raise ValueError("library directory escapes challenge")
        for path in library.rglob("*"):
            if path.is_file() and path.relative_to(base).as_posix() not in spec["files"]:
                raise ValueError("unrecorded library in LaunchSpec")
    source_digest=digest(target)
    derived_runtime = loader is not None
    if loader:
        target=patched_runtime(base,spec)
        loader=None
    executed_digest=digest(target)
    experiment=None
    if isolated:
        import isolation
        experiment=isolation.sandbox(root,isolation.allocate_uid(root))
        target=isolation.stage_file(target,experiment,"target.bin",True,
                                    derived_sha256=executed_digest if derived_runtime else None)
        # Preserve relative data files and the configured working directory.
        original_cwd=cwd
        staged_cwd=isolation.stage_directory(cwd,experiment,"cwd")
        cwd=Path(experiment["work"])/"cwd"
        staged_cwd.rename(cwd)
        for item in [cwd,*cwd.rglob("*")]:
            executable=item.is_dir() or (original_cwd/item.relative_to(cwd)).stat().st_mode&0o111
            item.chmod(0o755 if executable else 0o644)
            os.chown(item,experiment["uid"],experiment["uid"])
        if loader:
            library=isolation.stage_directory(library,experiment,"libraries")
            loader=library/Path(spec["loader"]).name; loader.chmod(0o555)
        if input_path: input_path=isolation.stage_file(input_path,experiment,"input.bin")
        isolation.seal(experiment)
    args=list(spec["args"])
    if spec["input_mode"]=="argv_file":
        if input_path is None: raise ValueError("argv_file launch needs an input file")
        args=[str(input_path) if a=="@@" else a for a in args]
    argv=([str(loader),"--library-path",str(library)] if loader else [])+[str(target)]+args
    return {"argv":argv,"target":target,"args":args,"cwd":cwd,"env":env,"experiment":experiment,
            "loader":loader,"library_path":library,"input_mode":spec["input_mode"],"stdin_path":input_path,
            "identity":hashlib.sha256(json.dumps(spec,sort_keys=True).encode()).hexdigest(),"timeout_secs":spec["timeout_secs"],"source_target_sha256":source_digest,"executed_target_sha256":executed_digest}
