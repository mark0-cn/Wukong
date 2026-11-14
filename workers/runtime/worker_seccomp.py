"""Offline classic-BPF inspection; artifacts never prove installed filter state."""
import hashlib
import struct
from worker_common import confined

ACTIONS = {0: "KILL_THREAD", 0x80000000: "KILL_PROCESS", 0x30000: "TRAP",
           0x50000: "ERRNO", 0x7fc00000: "USER_NOTIF", 0x7ff00000: "TRACE",
           0x7ffc0000: "LOG", 0x7fff0000: "ALLOW"}
ALU = {0: "+", 0x10: "-", 0x20: "*", 0x30: "/", 0x40: "|", 0x50: "&",
       0x60: "<<", 0x70: ">>", 0x90: "%", 0xa0: "^"}
JUMPS = {0x10: "==", 0x20: ">", 0x30: ">=", 0x40: "&"}


def action(value):
    return {"value": value, "action": ACTIONS.get(value & 0xffff0000, "UNKNOWN_ACTION"), "data": value & 0xffff}


def field(offset, endian):
    if offset == 0: return "nr"
    if offset == 4: return "arch"
    half = "low32" if (offset % 8 == 0) == (endian == "little") else "high32"
    return ("instruction_pointer" if offset < 16 else "args[%d]" % ((offset - 16) // 8)) + "." + half


def decode(data, endian="little"):
    if endian not in {"little", "big"}: raise ValueError("invalid filter endianness")
    if not data or len(data) % 8 or len(data) > 32768:
        raise ValueError("filter must contain 1..4096 raw sock_filter instructions (8 bytes each)")
    program = list(struct.iter_unpack(("<" if endian == "little" else ">") + "HBBI", data))
    rows = []
    for pc, (code, jt, jf, k) in enumerate(program):
        row = {"pc": pc, "code": code, "jt": jt, "jf": jf, "k": k, "supported": True}
        source = "X" if code & 8 else str(k)
        if code in {0x20, 0x80}:
            if code == 0x20 and (k % 4 or k > 60): raise ValueError("invalid seccomp_data word offset")
            row["operation"] = "A = " + (field(k, endian) if code == 0x20 else "64")
        elif code in {0, 1}: row["operation"] = ("X" if code else "A") + " = " + str(k)
        elif code in {0x60, 0x61, 2, 3}:
            if k >= 16: raise ValueError("BPF scratch index exceeds 15")
            reg = "X" if code in {0x61, 3} else "A"
            row["operation"] = (reg + " = M[%d]" % k) if code in {0x60, 0x61} else ("M[%d] = " % k + reg)
        elif code == 0x84: row["operation"] = "A = -A (uint32)"
        elif code in {op | src | 4 for op in ALU for src in (0, 8)}:
            row["operation"] = "A = A %s %s (uint32)" % (ALU[code & 0xf0], source)
            if not code & 8 and ((code & 0xf0 in {0x30, 0x90} and k == 0) or (code & 0xf0 in {0x60, 0x70} and k >= 32)):
                raise ValueError("invalid BPF arithmetic constant")
        elif code == 5: row.update(operation="jump", successors=[pc + 1 + k])
        elif code in {op | src | 5 for op in JUMPS for src in (0, 8)}:
            row.update(operation="A %s %s" % (JUMPS[code & 0xf0], source), successors=[pc+1+jt, pc+1+jf])
        elif code in {6, 0x16}:
            row.update(operation="return " + ("A" if code == 0x16 else str(k)), successors=[])
            if code == 6: row["decision"] = action(k)
        elif code in {7, 0x87}: row["operation"] = "X = A" if code == 7 else "A = X"
        else: row.update(operation="unsupported opcode", supported=False)
        row.setdefault("successors", [pc+1])
        if any(target >= len(program) for target in row["successors"]): raise ValueError("filter jumps or falls outside the program")
        rows.append(row)
    return program, rows


def probe(program, rows, query, endian):
    arch, nr = query["arch"], query["nr"]
    args, ip = query.get("args") or [], query.get("instruction_pointer") or 0
    if (any(not isinstance(v, int) or isinstance(v, bool) or not 0 <= v <= 0xffffffff for v in (arch, nr))
            or len(args) > 6 or any(not isinstance(v, int) or isinstance(v, bool) or not 0 <= v < 2**64 for v in [ip, *args])):
        raise ValueError("probe needs uint32 arch/nr, uint64 IP and at most six uint64 arguments")
    data = struct.pack(("<" if endian == "little" else ">") + "II7Q", nr, arch, ip, *(args+[0]*(6-len(args))))
    a = x = pc = 0
    memory, trace = {}, []
    for _ in range(len(program)):
        trace.append(pc)
        code, jt, jf, k = program[pc]
        if not rows[pc]["supported"]: return {"status": "unknown", "reason": "unsupported opcode", "trace": trace}
        value = x if code & 8 else k
        if code == 0x20: a = int.from_bytes(data[k:k+4], endian)
        elif code == 0x80: a = 64
        elif code == 0: a = k
        elif code == 1: x = k
        elif code in {0x60, 0x61}:
            if k not in memory: return {"status": "unknown", "reason": "uninitialized scratch word", "trace": trace}
            if code == 0x60: a = memory[k]
            else: x = memory[k]
        elif code in {2, 3}: memory[k] = a if code == 2 else x
        elif code in {6, 0x16}: return {"status": "evaluated", "decision": action(k if code == 6 else a), "trace": trace}
        elif code == 7: x = a
        elif code == 0x87: a = x
        elif code & 7 == 4:
            op = code & 0xf0
            if (op in {0x30, 0x90} and value == 0) or (op in {0x60, 0x70} and value >= 32):
                return {"status": "unknown", "reason": "undefined arithmetic operand", "trace": trace}
            if op == 0x80: a = -a
            elif op == 0: a += value
            elif op == 0x10: a -= value
            elif op == 0x20: a *= value
            elif op == 0x30: a //= value
            elif op == 0x40: a |= value
            elif op == 0x50: a &= value
            elif op == 0x60: a <<= value
            elif op == 0x70: a >>= value
            elif op == 0x90: a %= value
            elif op == 0xa0: a ^= value
            a &= 0xffffffff
        elif code == 5:
            pc += 1+k
            continue
        elif code & 7 == 5:
            op = code & 0xf0
            yes = {0x10: a == value, 0x20: a > value, 0x30: a >= value, 0x40: bool(a & value)}[op]
            pc += 1+(jt if yes else jf)
            continue
        pc += 1
    return {"status": "unknown", "reason": "no return reached", "trace": trace}


class SeccompService:
    def __init__(self, root, trusted, isolated): self.root = root

    def dispatch(self, method, params):
        path = confined(self.root, params["filter"], True)
        if path.stat().st_size > 32768: raise ValueError("filter exceeds 32 KiB")
        data = path.read_bytes()
        endian = params.get("endianness") or "little"
        program, rows = decode(data, endian)
        queries = params.get("probes") or []
        if len(queries) > 64: raise ValueError("at most 64 concrete probes")
        return {"filter_sha256": hashlib.sha256(data).hexdigest(), "endianness": endian,
                "instructions": rows, "supported": all(row["supported"] for row in rows),
                "probes": [{"query": q, **probe(program, rows, q, endian)} for q in queries],
                "runtime_binding": "unconfirmed_artifact", "limits": [
                    "Concrete probes are not an exhaustive syscall allowlist; argument, IP and architecture branches matter.",
                    "Installed filter stacks, user-notification responses and kernel availability are not observed."]}
