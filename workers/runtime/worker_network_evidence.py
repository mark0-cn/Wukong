"""Original wire chunks and trial evidence, independent of the replay policy."""
import base64
from worker_common import atomic_json, sha256_file


class Transcript:
    def __init__(self):
        self.events = []
        self.steps = []

    def step(self, index, direction):
        event = {"step": index, "direction": direction}
        self.steps.append(event)
        return event

    def wire(self, index, direction, data):
        self.events.append({"step": index, "direction": direction,
                            "data_base64": base64.b64encode(data).decode()})

    def save(self, root, trial, binding, captures, result, pending):
        if pending:
            self.steps.append({"direction": "receive_pending", "data_base64": base64.b64encode(pending).decode()})
        path = trial / "network-transcript.json"
        atomic_json(path, {"binding": binding, "events": self.events, "steps": self.steps,
                           "captures": captures, "result": result})
        result["transcript_sha256"] = sha256_file(path)
        result["transcript_path"] = path.relative_to(root).as_posix()
