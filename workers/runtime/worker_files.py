"""Files."""
import base64
import threading
from worker_common import MAX_FILE, artifact, bounded_int, confined, public_file

class FileService:

    def __init__(self, root, trusted, isolated):
        self.root, self.trusted, self.isolated = root, trusted, isolated
        self.lock = threading.RLock()
        (root / "files").mkdir(exist_ok=True)

    def dispatch(self, method, params):
        if method == "file.put":
            path = confined(self.root, "files/" + str(params["path"]))
            data = base64.b64decode(params["data_base64"], validate=True)
            if len(data) > MAX_FILE:
                raise ValueError("file upload exceeds 32 MiB")
            path.parent.mkdir(parents=True, exist_ok=True)
            if path.exists():
                if path.is_file() and path.read_bytes() == data:
                    return artifact(self.root, path)
                raise ValueError("file path already exists with different content; choose another path")
            with path.open("xb") as stream:
                stream.write(data)
            path.chmod(0o555 if params.get("executable") else 0o444)
            return artifact(self.root, path)
        if method == "file.get":
            path = public_file(self.root, params["path"])
            maximum = bounded_int(params.get("max_bytes"), 1048576, 1, MAX_FILE)
            if not path.is_file() or path.stat().st_size > maximum:
                raise ValueError("file exceeds max_bytes or is not a regular file")
            return {**artifact(self.root, path), "data_base64": base64.b64encode(path.read_bytes()).decode()}
        raise ValueError("unsupported FileService method: " + method)
