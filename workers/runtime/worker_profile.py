"""Bounded runtime identity and immutable loader/libc bundles; never executes inspected files."""
import hashlib
import json
import os
from pathlib import Path
import threading
from worker_common import MAX_FILE, artifact, confined


def file_identity(path, expected_inode=None):
    path = Path(path)
    with path.open("rb") as stream:
        stat = os.fstat(stream.fileno())
        if expected_inode and (os.major(stat.st_dev), os.minor(stat.st_dev), stat.st_ino) != expected_inode:
            raise ValueError("mapped file identity changed")
        if stat.st_size > MAX_FILE: raise ValueError("identity file exceeds 32 MiB")
        data = stream.read(MAX_FILE + 1)
        if len(data) > MAX_FILE: raise ValueError("identity file exceeds 32 MiB")
        after = os.fstat(stream.fileno())
        if (stat.st_size,stat.st_mtime_ns,stat.st_ctime_ns) != (after.st_size,after.st_mtime_ns,after.st_ctime_ns):
            raise ValueError("file changed during identity observation")
    result = {"path":str(path), "sha256":hashlib.sha256(data).hexdigest(), "size":len(data),
              "build_id":None, "architecture":None, "bits":None, "interpreter":None}
    if data[:4] == b"\x7fELF":
        try:
            from elftools.elf.elffile import ELFFile
            import io
            elf = ELFFile(io.BytesIO(data))
            result.update(architecture=elf.get_machine_arch(), bits=elf.elfclass)
            from worker_mitigations import declared_cet
            result["elf_properties"] = declared_cet(elf)
            for segment in elf.iter_segments():
                if segment["p_type"] == "PT_INTERP": result["interpreter"] = segment.get_interp_name()
                if segment["p_type"] == "PT_NOTE":
                    for note in segment.iter_notes():
                        if note["n_type"] == "NT_GNU_BUILD_ID": result["build_id"] = str(note["n_desc"])
        except Exception as error:
            result["elf_metadata_error"] = str(error)[:256]
    else:
        result["elf_metadata_error"] = "not ELF"
    return result


def parse_maps(text):
    rows = []
    for line in text.splitlines()[:4096]:
        fields = line.split(None, 5)
        if len(fields) < 5: continue
        start, end = (int(value, 16) for value in fields[0].split("-"))
        major, minor = (int(value, 16) for value in fields[3].split(":"))
        rows.append({"start":start,"end":end,"permissions":fields[1],"offset":int(fields[2],16),
            "device":[major,minor],"inode":int(fields[4]),"path":fields[5] if len(fields)>5 else None})
    return rows


def process_profile(pid):
    # PID is supplied only by the owning debugger session, never by a model argument.
    proc = Path("/proc") / str(int(pid))
    def identity():
        text = (proc / "stat").read_text()
        return text[text.rfind(")")+2:].split()[19]
    before = identity()
    with (proc / "maps").open() as stream: raw = stream.read(1048577)
    if len(raw) > 1048576: raise ValueError("process maps exceed limit")
    maps = parse_maps(raw)
    modules, seen, total = [], set(), 0
    for row in maps:
        path = row["path"]
        key = (*row["device"], row["inode"])
        if not path or not path.startswith("/") or key in seen: continue
        seen.add(key)
        if len(modules) >= 64: break
        module = {"mapped_path":path,"inode":row["inode"],"device":row["device"],"sha256":None,"build_id":None}
        try:
            if path.endswith(" (deleted)"): raise ValueError("mapped file deleted")
            candidate = proc / "root" / path.lstrip("/")
            total += candidate.stat().st_size
            if total > 64*1024*1024: raise ValueError("module identity byte budget exceeded")
            module.update(file_identity(candidate, key))
        except (OSError, ValueError) as error: module["identity_error"] = str(error)[:256]
        modules.append(module)
    from worker_mitigations import thread_observations, read_bounded
    protections = thread_observations(proc)
    if before != identity(): raise ValueError("process identity changed during observation")
    if read_bounded(proc / "maps",1048576) != raw: raise ValueError("process mappings changed during observation; retry at a stable stop")
    return {"pid":int(pid),"start_time_ticks":before,"maps":maps,"modules":modules,
            "maps_text":raw,"status":"observed","complete_modules":len(seen)<=64, "protections":protections}


class ProfileService:
    def __init__(self, root, trusted, isolated):
        self.root = Path(root).resolve()
        self.lock = threading.RLock()

    def profile(self, params):
        identities = {}
        for key in ["target", "libc", "loader"]:
            if params.get(key): identities[key] = file_identity(confined(self.root, params[key], True))
        if "target" not in identities: raise ValueError("target required")
        from worker_mitigations import environment_support
        result = {"environment_support":environment_support(), "files":identities,"status":"declared_files_only","mapped_match":False,
                  "note":"File identity is not proof that the target loaded this runtime."}

        return result

    def prepare(self, params):
        result = self.profile(params)
        identities = result["files"]
        if set(identities) != {"target", "libc", "loader"}: raise ValueError("prepare requires target, libc and loader")
        if any(value.get("elf_metadata_error") for value in identities.values()):
            raise ValueError("prepare requires ELF metadata support and three ELF files")
        if len({(v["architecture"],v["bits"]) for v in identities.values()}) != 1:
            raise ValueError("runtime architecture mismatch")
        digest = hashlib.sha256(json.dumps({k:v["sha256"] for k,v in identities.items()},sort_keys=True).encode()).hexdigest()
        bundle = confined(self.root, "files/runtime-" + digest)
        names = {"target":"target.bin", "libc":"lib/libc.so.6", "loader":"loader"}
        files = []
        with self.lock:
            for key, name in names.items():
                destination = bundle / name
                destination.parent.mkdir(parents=True, exist_ok=True)
                data = Path(identities[key]["path"]).read_bytes()
                if hashlib.sha256(data).hexdigest() != identities[key]["sha256"]: raise ValueError("source changed")
                if destination.exists():
                    if destination.read_bytes() != data: raise ValueError("immutable runtime bundle conflict")
                else:
                    with destination.open("xb") as stream: stream.write(data)
                    destination.chmod(0o444 if key == "libc" else 0o555)
                files.append(artifact(self.root,destination))
        result.update(bundle_id=digest,artifacts=files,
            launch_argv=[str(bundle/"loader"),"--library-path",str(bundle/"lib"),str(bundle/"target.bin")],
            note="Run these direct argv entries through job_start or interactive_start. Original files are unchanged. Recheck actual mappings; dependencies beyond libc may still be missing.")
        return result
