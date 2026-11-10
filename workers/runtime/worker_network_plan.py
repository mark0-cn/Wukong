"""Bounded declarative TCP plans and per-connection captured values."""
import base64
import re

LIMIT = 1024 * 1024


def blob(value):
    if not isinstance(value, str) or not value or len(value) > 4*LIMIT//3+4:
        raise ValueError("nonempty bounded base64 bytes required")
    result = base64.b64decode(value, validate=True)
    if not result or len(result) > LIMIT: raise ValueError("decoded bytes exceed bounds")
    return result


def uint(value, maximum=LIMIT):
    if not isinstance(value, int) or isinstance(value, bool) or not 0 <= value <= maximum:
        raise ValueError("integer outside protocol bounds")
    return value


def validate_plan(plan, input_bytes):
    if not isinstance(plan, dict) or set(plan) != {"steps"}: raise ValueError("network plan needs only steps")
    steps = plan["steps"]
    if not isinstance(steps, list) or not 2 <= len(steps) <= 64 or len(input_bytes) > LIMIT:
        raise ValueError("network replay supports 2..64 steps and input <=1 MiB")
    captures, received, sent = set(), False, False
    for step in steps:
        op = step.get("op")
        allowed = {"send":{"op","offset","length","patches"}, "receive":{"op","until_base64","max_bytes"},
                   "capture":{"op","name","offset","length","encoding"}, "expect":{"op","contains_base64"}}
        if op not in allowed or set(step)-allowed[op]: raise ValueError("unsupported network step or field")
        if op == "send":
            offset, length = uint(step["offset"]), uint(step["length"])
            if not length or offset+length > len(input_bytes): raise ValueError("send slice exceeds input")
            patches = step.get("patches", [])
            if not isinstance(patches,list) or len(patches) > 32: raise ValueError("too many or invalid send patches")
            for patch in patches:
                if set(patch)-{"offset","capture","addend","width","endianness"}: raise ValueError("invalid patch field")
                if patch["capture"] not in captures or uint(patch["width"],8) not in {1,2,4,8} or patch["endianness"] not in {"little","big"}:
                    raise ValueError("invalid capture patch")
                if uint(patch["offset"])+patch["width"] > length: raise ValueError("patch exceeds send slice")
                addend=patch.get("addend",0)
                if not isinstance(addend,int) or isinstance(addend,bool) or not -(2**63) <= addend < 2**63: raise ValueError("patch addend must be int64")
            sent, received = True, False
        elif op == "receive":
            if not 1 <= uint(step["max_bytes"]) <= 65536 or len(blob(step["until_base64"])) > step["max_bytes"]:
                raise ValueError("receive delimiter exceeds step budget")
            received = True
        elif op == "capture":
            if not received or not re.fullmatch(r"[A-Za-z][A-Za-z0-9_]{0,31}",step["name"]) or step["name"] in captures:
                raise ValueError("capture needs a unique name and preceding receive")
            if not 1 <= uint(step["length"],32) or step["encoding"] not in {"hex","little","big"}: raise ValueError("invalid capture encoding/length")
            uint(step["offset"])
            if step["encoding"] != "hex" and step["length"] > 8: raise ValueError("integer capture exceeds uint64")
            captures.add(step["name"])
        else:
            if not sent or not received: raise ValueError("expect requires a response after sending input")
            blob(step["contains_base64"])
    if steps[-1]["op"] != "expect": raise ValueError("final network step must assert received bytes")


class ReplayState:
    """Captures belong to one connection and never survive between trials."""
    def __init__(self):
        self.captures = {}
        self.received = None
        self.fresh = None

    def payload(self, step, input_bytes):
        data = bytearray(input_bytes[step["offset"]:step["offset"]+step["length"]])
        for patch in step.get("patches", []):
            value = self.captures[patch["capture"]] + patch.get("addend", 0)
            width = patch["width"]
            if not 0 <= value < 2**(8*width):
                raise ValueError("patched address does not fit; no implicit wraparound")
            data[patch["offset"]:patch["offset"]+width] = value.to_bytes(width, patch["endianness"])
        return data

    def capture(self, step):
        data = self.received[step["offset"]:step["offset"]+step["length"]]
        if len(data) != step["length"]:
            raise ValueError("capture exceeds response")
        if step["encoding"] == "hex":
            text = data.decode("ascii")
            if not re.fullmatch(r"(?:0x)?[0-9a-fA-F]+", text):
                raise ValueError("capture is not hexadecimal")
            value = int(text, 16)
        else:
            value = int.from_bytes(data, step["encoding"])
        if value >= 2**64:
            raise ValueError("capture exceeds uint64")
        self.captures[step["name"]] = value

    def expect(self, step):
        if blob(step["contains_base64"]) not in self.fresh:
            raise ValueError("fresh response bytes do not match expectation")
