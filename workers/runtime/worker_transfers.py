"""Resumable, bounded file transfer; uploads publish immutable hash-checked artifacts."""
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import shutil

from filelock import FileLock
from worker_common import artifact, atomic_json, confined, load_json, public_file, sha256_file

CHUNK_BYTES = 1024 * 1024
MAX_TRANSFER_BYTES = 16 * 1024**3


def integer(value, minimum, maximum):
    if type(value) is not int or not minimum <= value <= maximum:
        raise ValueError("transfer integer outside permitted range")
    return value


class TransferService:

    def __init__(self, root):
        self.root = Path(root).resolve()
        (self.root / "transfers").mkdir(exist_ok=True)

    def directory(self, transfer_id):
        if not isinstance(transfer_id, str) or not re.fullmatch(r"[0-9a-f]{64}", transfer_id):
            raise ValueError("invalid transfer ID")
        return confined(self.root, "transfers/" + transfer_id, True)

    def status(self, directory, spec):
        destination = confined(self.root, "files/" + spec["path"])
        if destination.exists():
            if not destination.is_file() or destination.stat().st_size != spec["size"] or sha256_file(destination) != spec["sha256"]:
                raise ValueError("destination exists with different content")
            if os.name == "posix" and bool(destination.stat().st_mode & 0o111) != spec["executable"]:
                raise ValueError("destination has a different executable policy")
            return {"transfer_id": directory.name, "offset": spec["size"], "complete": True,
                    "artifact": artifact(self.root, destination)}
        data = directory / "data"
        offset = data.stat().st_size if data.exists() else 0
        if offset > spec["size"]:
            raise ValueError("staging data exceeds declared size")
        return {"transfer_id": directory.name, "offset": offset, "complete": False}

    def dispatch(self, method, params):
        if method == "file.stat":
            path = public_file(self.root, params["path"])
            result = artifact(self.root, path)
            if result is None:
                raise ValueError("artifact is not a regular file")
            return result
        if method == "file.read":
            path = public_file(self.root, params["path"])
            if not path.is_file():
                raise ValueError("artifact is not a regular file")
            size = path.stat().st_size
            offset = integer(params.get("offset"), 0, size)
            maximum = integer(params.get("max_bytes", CHUNK_BYTES), 1, CHUNK_BYTES)
            with path.open("rb") as stream:
                stream.seek(offset)
                data = stream.read(maximum)
            return {"path": path.relative_to(self.root).as_posix(), "offset": offset,
                    "size": size, "data_base64": base64.b64encode(data).decode(),
                    "chunk_sha256": hashlib.sha256(data).hexdigest()}
        if method == "file.upload.begin":
            path = str(params["path"])
            if any(character in path for character in ("\\", ":", "\0")):
                raise ValueError("upload paths must use relative POSIX components")
            destination = confined(self.root, "files/" + path)
            if destination == self.root / "files":
                raise ValueError("file path is required")
            digest = params["sha256"]
            if not isinstance(digest, str) or not re.fullmatch(r"[0-9a-f]{64}", digest):
                raise ValueError("invalid file SHA256")
            executable = params.get("executable", False)
            if type(executable) is not bool:
                raise ValueError("executable must be a boolean")
            spec = {"path": destination.relative_to(self.root / "files").as_posix(),
                    "size": integer(params["size"], 0, MAX_TRANSFER_BYTES),
                    "sha256": digest, "executable": executable}
            transfer_id = hashlib.sha256(json.dumps(spec, sort_keys=True).encode()).hexdigest()
            directory = self.root / "transfers" / transfer_id
            directory.mkdir(exist_ok=True)
            with FileLock(str(directory / "lock"), timeout=30):
                metadata = directory / "manifest.json"
                if metadata.exists() and load_json(metadata) != spec:
                    raise ValueError("transfer identity collision")
                if not metadata.exists():
                    atomic_json(metadata, spec)
                status = self.status(directory, spec)
                if not status["complete"] and shutil.disk_usage(self.root).free < spec["size"] - status["offset"]:
                    raise ValueError("insufficient disk space for upload")
                return {"transfer_id": transfer_id, "chunk_bytes": CHUNK_BYTES}
        directory = self.directory(params["transfer_id"])
        with FileLock(str(directory / "lock"), timeout=30):
            spec = load_json(directory / "manifest.json")
            status = self.status(directory, spec)
            if method == "file.upload.status":
                return status
            if method == "file.upload.write":
                offset = integer(params["offset"], 0, spec["size"])
                encoded = params["data_base64"]
                if not isinstance(encoded, str) or len(encoded) > 4 * ((CHUNK_BYTES + 2) // 3):
                    raise ValueError("upload chunk exceeds 1 MiB")
                data = base64.b64decode(encoded, validate=True)
                if not data or len(data) > CHUNK_BYTES or offset + len(data) > spec["size"]:
                    raise ValueError("invalid upload chunk size")
                if status["complete"]:
                    path = confined(self.root, "files/" + spec["path"], True)
                    with path.open("rb") as stream:
                        stream.seek(offset)
                        if stream.read(len(data)) != data:
                            raise ValueError("duplicate chunk has different content")
                    return status
                if offset > status["offset"]:
                    raise ValueError("upload chunks must be contiguous")
                staging = directory / "data"
                with staging.open("r+b" if staging.exists() else "w+b") as stream:
                    stream.seek(offset)
                    existing = stream.read(min(len(data), status["offset"] - offset))
                    if existing != data[:len(existing)]:
                        raise ValueError("duplicate chunk has different content")
                    stream.seek(offset + len(existing))
                    stream.write(data[len(existing):])
                    stream.flush()
                    os.fsync(stream.fileno())
                return {"transfer_id": directory.name, "offset": max(status["offset"], offset + len(data)), "complete": False}
            if method == "file.upload.commit":
                if status["complete"]:
                    return status["artifact"]
                staging = directory / "data"
                if not staging.exists() and spec["size"] == 0:
                    staging.touch()
                if not staging.is_file() or staging.stat().st_size != spec["size"] or sha256_file(staging) != spec["sha256"]:
                    raise ValueError("upload is incomplete or SHA256 does not match")
                destination = confined(self.root, "files/" + spec["path"])
                destination.parent.mkdir(parents=True, exist_ok=True)
                staging.chmod(0o555 if spec["executable"] else 0o444)
                try:
                    # Atomic create-if-absent; concurrent transfers cannot overwrite a file.
                    os.link(staging, destination)
                except FileExistsError:
                    return self.status(directory, spec)["artifact"]
                return artifact(self.root, destination)
        raise ValueError("unsupported transfer method: " + method)
