"""Linux credential boundary between trusted worker metadata and experiments."""
import base64
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import uuid


def ensure_uid_entries(start=20000, count=512):
    """Register the allocator's UID range in /etc/passwd once per container.

    Python tooling (pyvex/getpass and others) resolves the current uid via
    pwd.getpwuid; a numeric uid without a passwd entry breaks them inside
    isolated jobs. The daemon runs as root in isolated mode and performs the
    one-time registration.
    """
    if os.geteuid() != 0:
        return
    marker = "/etc/wukong-passwd.ready"
    if os.path.exists(marker):
        return
    with open("/etc/passwd", "a", encoding="utf-8") as stream:
        for uid in range(start, start + count):
            stream.write("wukong-%d:x:%d:%d::/nonexistent:/bin/false\n" % (uid, uid, uid))
    with open(marker, "w", encoding="utf-8"):
        pass


def allocate_uid(root):
    import fcntl
    lock = root / "uid-allocator.lock"
    with lock.open("a+b") as stream:
        os.chmod(lock, 0o600)
        fcntl.flock(stream, fcntl.LOCK_EX)
        path = root / "uid-allocator.json"
        value = json.loads(path.read_text())["next"] if path.exists() else 20000
        if value >= 1000000000:
            raise ValueError("experiment UID allocator exhausted")
        temporary = path.with_suffix(".tmp")
        temporary.write_text(json.dumps({"next": value + 1}))
        temporary.chmod(0o600)
        os.replace(temporary, path)
        ensure_uid_entries()
        # The batch registration covers only the first 512 allocations; the
        # allocator itself is unbounded. Register every allocated uid so
        # pwd.getpwuid-based tooling (pyvex/getpass) keeps working forever.
        try:
            import pwd
            pwd.getpwuid(value)
        except (ImportError, KeyError):
            with open("/etc/passwd", "a", encoding="utf-8") as entries:
                entries.write("wukong-%d:x:%d:%d::/nonexistent:/bin/false\n"
                              % (value, value, value))
        return value


def sandbox(root, uid):
    namespace = hashlib.sha256(str(root).encode()).hexdigest()[:24]
    parent = Path("/tmp/wukong-executions") / namespace
    parent.mkdir(parents=True, exist_ok=True)
    parent.chmod(0o711)
    directory = parent / uuid.uuid4().hex
    directory.mkdir(mode=0o711)
    readonly = directory / "readonly"
    readonly.mkdir(mode=0o755)
    work = directory / "work"
    work.mkdir(mode=0o700)
    try:
        os.chown(work, uid, uid)
    except PermissionError as error:
        raise ValueError("isolated worker requires CAP_CHOWN, SETUID and SETGID to own experiment directories") from error
    return {"root": str(directory), "readonly": str(readonly), "work": str(work), "uid": uid,
            "state_root": str(Path(root).resolve())}


def _public_source(source, experiment, derived_sha256=None):
    from worker_common import public_file, sha256_file
    state = Path(experiment["state_root"]).resolve()
    source = Path(source).resolve()
    if source != state and state not in source.parents:
        # Only the launch resolver supplies this hash, after validating public
        # source artifacts and the immutable prepared-runtime cache. A digest
        # never authorizes worker metadata or an arbitrary external path.
        cache = Path('/tmp/wukong-launches')
        try:
            member = source.relative_to(cache).parts
        except ValueError:
            member = ()
        if (derived_sha256 is None or len(member) != 2 or member[1] != 'target'
                or len(member[0]) != 64 or any(c not in '0123456789abcdef' for c in member[0])):
            raise ValueError("staged input must be a public worker artifact")
        for path in [cache, source.parent, source]:
            status = path.lstat()
            if path.is_symlink() or status.st_uid != os.geteuid() or status.st_mode & 0o022:
                raise ValueError('unsafe prepared launch input')
        if not source.is_file() or sha256_file(source) != derived_sha256:
            raise ValueError('prepared launch input identity changed')
        return source
    return public_file(state, source.relative_to(state).as_posix())


def stage_file(source, experiment, name=None, executable=False, *, derived_sha256=None):
    source = _public_source(source, experiment, derived_sha256)
    destination = Path(experiment["readonly"]) / (name or (uuid.uuid4().hex + "-" + source.name))
    if not destination.is_relative_to(Path(experiment['readonly'])) or '..' in Path(name or source.name).parts:
        raise ValueError('staged file name must stay inside readonly storage')
    destination.parent.mkdir(parents=True,exist_ok=True)
    shutil.copyfile(source, destination)
    destination.chmod(0o555 if executable else 0o444)
    return destination


def stage_directory(source, experiment, name):
    source = _public_source(source, experiment)
    destination = Path(experiment["readonly"]) / name
    count, total = 0, 0
    approved = []
    for item in sorted(source.rglob("*")):
        if item.is_symlink():
            raise ValueError("symlinks are not accepted in staged input directories")
        if not item.is_file():
            continue
        count += 1
        total += item.stat().st_size
        if count > 1024 or total > 32 * 1048576:
            raise ValueError("staged directory exceeds 1024 files/32 MiB")
        _public_source(item, experiment)
        approved.append(item)
    # Validate the complete tree before copying anything into target-readable
    # storage. In particular, never turn a job metadata directory into input.
    destination.mkdir(mode=0o755)
    for item in approved:
        path = destination / item.relative_to(source)
        path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(item, path)
        path.chmod(0o444)
    for directory in sorted(destination.rglob("*"), reverse=True):
        if directory.is_dir():
            directory.chmod(0o555)
    destination.chmod(0o555)
    return destination


def seal(experiment):
    Path(experiment["readonly"]).chmod(0o555)


def credentials(experiment):
    return {"user": experiment["uid"], "group": experiment["uid"], "extra_groups": [], "umask": 0o077}


def collect(root, experiment, destination, mode, env):
    """Read child-owned0700 output through a confined helper with the same UID.

    The helper is immutable trusted code; it has no access to worker metadata.
    Collected target bytes remain untrusted artifacts, never job authority.
    """
    source = Path(experiment["work"]) / "fuzz" if mode == "fuzz" else Path(experiment["work"])
    result = subprocess.run([sys.executable, str(Path(__file__).with_name("collect.py")), str(source), mode],
                            stdin=subprocess.DEVNULL, capture_output=True, env=env, timeout=15, **credentials(experiment))
    if result.returncode != 0 or len(result.stdout) > 24 * 1048576:
        raise ValueError("experiment artifact collector failed")
    data = json.loads(result.stdout)
    destination.mkdir(parents=True, exist_ok=True)
    artifacts = []
    for item in data.get("files", []):
        relative = Path(item["path"])
        if relative.is_absolute() or ".." in relative.parts:
            raise ValueError("invalid collected artifact path")
        path = destination / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        payload = base64.b64decode(item["data"], validate=True)
        if len(payload) > 1048576:
            raise ValueError("collected artifact too large")
        path.write_bytes(payload)
        artifacts.append(path)
    return data.get("stats", {}), artifacts
