"""Bounded Unicorn emulation for VM and hand-crafted ELF analysis.

`emu.run` executes the target from a start address under the Unicorn
emulator, mapping the ELF's load segments as writable memory plus a fixed
stack, and returns registers and memory dumps. It is a pure simulation: the
target never runs natively and no worker state is touched. Instruction
count, a stop address and a wall-clock watchdog bound the run.
"""
import base64
import json
import struct
import threading
import time

from worker_common import bounded_int, confined, parse_address, WorkerError

X86_64_REGISTERS = ["rax", "rbx", "rcx", "rdx", "rsi", "rdi", "rbp", "rsp", "rip",
                    "r8", "r9", "r10", "r11", "r12", "r13", "r14", "r15", "eflags"]
X86_32_REGISTERS = ["eax", "ebx", "ecx", "edx", "esi", "edi", "ebp", "esp", "eip", "eflags"]


# ---------------------------------------------------------------------------
# Exact x87 extended-precision semantics.
# Unicorn approximates the x87 stack in host double precision, which shifts
# the float32 roundings of chains like fldpi/fmulp (the CSAW "odd" decode
# loop). These helpers perform the involved operations in true 80-bit
# extended precision with Python integers, so float32 stores and fisttp
# truncations match the silicon exactly.
# A value is (sign, exponent, mantissa): value = (-1)^sign * mantissa * 2^(exponent - 63),
# mantissa is a 64-bit integer whose top bit is the explicit integer bit.
X87_PI = (0, 1, 0xC90FDAA22168C235)


def _f80_normalize(sign, exponent, mantissa):
    excess = mantissa.bit_length() - 64
    if excess > 0:
        keep = mantissa >> excess
        dropped = mantissa & ((1 << excess) - 1)
        halfway = 1 << (excess - 1)
        if dropped > halfway or (dropped == halfway and keep & 1):
            keep += 1
            if keep.bit_length() > 64:
                keep >>= 1
                exponent += 1
        mantissa = keep
        exponent += excess
    elif excess < 0:
        mantissa <<= -excess
        exponent -= -excess
    return sign, exponent, mantissa


def _f80_mul(a, b):
    # value = mantissa * 2^(exponent - 63): the raw product carries the sum
    # of the two (exponent-63) shifts, plus one 63 back to the representation.
    return _f80_normalize(a[0] ^ b[0], a[1] + b[1] - 63, a[2] * b[2])


def _f80_to_f32(value):
    """Round an 80-bit value to the IEEE-754 float32 bit pattern (nearest-even)."""
    sign, exponent, mantissa = value
    # float32: 1.f * 2^e, 23 fraction bits; value = mantissa * 2^(exponent-63).
    shift = 63 - 23
    significand = mantissa >> shift
    dropped = mantissa & ((1 << shift) - 1)
    halfway = 1 << (shift - 1)
    if dropped > halfway or (dropped == halfway and significand & 1):
        significand += 1
    # significand is 24 bits; overflow bumps the exponent and rounds to 1.0*2^(e+1)
    if significand >= 1 << 24:
        significand >>= 1
        exponent += 1
    e = exponent + 127
    if e <= 0:
        return sign << 31  # subnormal underflow: floor to zero (x87 honors masked underflow)
    if e >= 255:
        return (sign << 31) | 0x7F800000
    return (sign << 31) | (e << 23) | (significand & 0x7FFFFF)


def _f80_trunc_i32(value):
    """Truncate an 80-bit value to int32, like fisttp (toward zero)."""
    sign, exponent, mantissa = value
    magnitude = mantissa << max(0, exponent - 63) if exponent >= 63 else mantissa >> (63 - exponent)
    if sign:
        magnitude = -magnitude
    if magnitude > 0x7FFFFFFF:
        return 0x80000000 if magnitude > 0 else -0x80000000
    if magnitude < -0x80000000:
        return -0x80000000
    return magnitude


def _segments(path):
    try:
        from pwn import ELF  # noqa: PLC0415 - heavy import
    except ImportError as error:
        raise ValueError("pwntools is not installed in this worker") from error
    elf = ELF(str(path), checksec=False)
    bits = elf.elfclass  # 32 or 64
    segments = []
    for segment in elf.segments:
        if segment.header.p_type != "PT_LOAD":
            continue
        address = segment.header.p_vaddr
        size = segment.header.p_memsz
        if address == 0 and bits == 32:
            address = 0x08048000
        segments.append((address, size, bytes(segment.data())))
    return bits, segments


class EmuService:
    methods = frozenset(["emu.run"])

    def __init__(self, root, trusted, isolated):
        self.root, self.trusted, self.isolated = root, trusted, isolated

    def dispatch(self, method, params):
        if method == "emu.run":
            return self.run(params)
        raise ValueError("unsupported EmuService method: " + method)

    def run(self, params):
        target = confined(self.root, params["target"], True)
        if not target.is_file():
            raise ValueError("emu target must be a staged file")
        try:
            from unicorn import UC_ARCH_X86, UC_HOOK_CODE, UC_MODE_32, UC_MODE_64, UC_PROT_ALL, Uc  # noqa: PLC0415
            from unicorn.x86_const import (  # noqa: PLC0415
                UC_X86_REG_EIP, UC_X86_REG_ESP, UC_X86_REG_RIP, UC_X86_REG_RSP,
                UC_X86_REG_RAX, UC_X86_REG_RBX, UC_X86_REG_RCX, UC_X86_REG_RDX,
                UC_X86_REG_RSI, UC_X86_REG_RDI, UC_X86_REG_RBP,
                UC_X86_REG_R8, UC_X86_REG_R9, UC_X86_REG_R10, UC_X86_REG_R11,
                UC_X86_REG_R12, UC_X86_REG_R13, UC_X86_REG_R14, UC_X86_REG_R15,
                UC_X86_REG_EFLAGS,
                UC_X86_REG_EAX, UC_X86_REG_EBX, UC_X86_REG_ECX, UC_X86_REG_EDX,
                UC_X86_REG_ESI, UC_X86_REG_EDI, UC_X86_REG_EBP,
            )
        except ImportError as error:
            raise ValueError("unicorn is not installed in this worker") from error
        bits, segments = _segments(target)
        mode = UC_MODE_64 if bits == 64 else UC_MODE_32
        start = parse_address(params["start"])
        count = bounded_int(params.get("count"), 10000, 1, 10_000_000)
        until = parse_address(params.get("until")) if params.get("until") else None
        if until is None and not count:
            raise ValueError("until or count is required")
        timeout_secs = bounded_int(params.get("timeout_secs"), 30, 1, 600)
        mu = Uc(UC_ARCH_X86, mode)
        stack_top = 0x7FFF0000 if bits == 64 else 0xFFFE0000
        mu.mem_map(stack_top - 0x800000, 0x800000, UC_PROT_ALL)
        for address, size, data in segments:
            aligned = address & ~0xFFF
            span = (address + max(size, 1) - aligned + 0xFFF) & ~0xFFF
            mu.mem_map(aligned, span, UC_PROT_ALL)
            if data:
                mu.mem_write(address, data)
        init = params.get("init_regs") or {}
        if bits == 64:
            table = {"rax": UC_X86_REG_RAX, "rbx": UC_X86_REG_RBX, "rcx": UC_X86_REG_RCX,
                     "rdx": UC_X86_REG_RDX, "rsi": UC_X86_REG_RSI, "rdi": UC_X86_REG_RDI,
                     "rbp": UC_X86_REG_RBP, "rsp": UC_X86_REG_RSP, "rip": UC_X86_REG_RIP,
                     "r8": UC_X86_REG_R8, "r9": UC_X86_REG_R9, "r10": UC_X86_REG_R10,
                     "r11": UC_X86_REG_R11, "r12": UC_X86_REG_R12, "r13": UC_X86_REG_R13,
                     "r14": UC_X86_REG_R14, "r15": UC_X86_REG_R15, "eflags": UC_X86_REG_EFLAGS}
        else:
            table = {"eax": UC_X86_REG_EAX, "ebx": UC_X86_REG_EBX, "ecx": UC_X86_REG_ECX,
                     "edx": UC_X86_REG_EDX, "esi": UC_X86_REG_ESI, "edi": UC_X86_REG_EDI,
                     "ebp": UC_X86_REG_EBP, "esp": UC_X86_REG_ESP, "eip": UC_X86_REG_EIP,
                     "eflags": UC_X86_REG_EFLAGS}
        if "rsp" not in init and "esp" not in init:
            mu.reg_write(UC_X86_REG_RSP if bits == 64 else UC_X86_REG_ESP, stack_top - 0x1000)
        for name, value in init.items():
            if not isinstance(value, int):
                raise ValueError("init_regs values must be integers")
            if name not in table:
                raise ValueError("unknown register for this mode: " + name)
            mu.reg_write(table[name], value)
        for write in params.get("memory") or []:
            mu.mem_write(parse_address(write["address"]),
                         base64.b64decode(write["data_base64"], validate=True))
        state = {"instructions": 0, "stopped": None, "error": None}
        x87_exact = bool(params.get("x87_exact", True))
        x87_stack = []  # Python-side 80-bit shadow of ST(0..7)
        pc_register = UC_X86_REG_RIP if bits == 64 else UC_X86_REG_EIP
        reg32 = {"eax": UC_X86_REG_EAX, "ecx": UC_X86_REG_ECX, "edx": UC_X86_REG_EDX,
                 "ebx": UC_X86_REG_EBX, "esp": UC_X86_REG_ESP, "ebp": UC_X86_REG_EBP,
                 "esi": UC_X86_REG_ESI, "edi": UC_X86_REG_EDI}
        reg64 = {"rax": UC_X86_REG_RAX, "rcx": UC_X86_REG_RCX, "rdx": UC_X86_REG_RDX,
                 "rbx": UC_X86_REG_RBX, "rsp": UC_X86_REG_RSP, "rbp": UC_X86_REG_RBP,
                 "rsi": UC_X86_REG_RSI, "rdi": UC_X86_REG_RDI}
        reg_by_index = {0: "rax", 1: "rcx", 2: "rdx", 3: "rbx", 4: "rsp", 5: "rbp", 6: "rsi", 7: "rdi"} \
            if bits == 64 else {0: "eax", 1: "ecx", 2: "edx", 3: "ebx", 4: "esp", 5: "ebp", 6: "esi", 7: "edi"}
        reg_table = reg64 if bits == 64 else reg32

        def step_x87(uc, address):
            """Intercept one x87 instruction; return its length, or None if uncovered."""
            raw = bytes(uc.mem_read(address, 4))
            opcode, modrm = raw[0], raw[1]
            if opcode == 0xD9 and modrm == 0xEB:  # fldpi
                x87_stack.append(X87_PI)
                return 2
            if opcode == 0xDE and modrm == 0xC9:  # fmulp st(1), st
                if len(x87_stack) < 2:
                    raise ValueError("fmulp with fewer than two x87 stack entries")
                right = x87_stack.pop()
                left = x87_stack.pop()
                x87_stack.append(_f80_mul(left, right))
                return 2
            if opcode in (0xD9, 0xDB) and (modrm >> 6) == 0:
                target_reg = reg_table[reg_by_index[modrm & 7]]
                address_base = uc.reg_read(target_reg)
                if opcode == 0xD9 and ((modrm >> 3) & 7) == 2:  # fst dword [reg]
                    uc.mem_write(address_base, struct.pack("<I", _f80_to_f32(x87_stack[-1])))
                    return 2
                if opcode == 0xDB and ((modrm >> 3) & 7) == 1:  # fisttp dword [reg]
                    uc.mem_write(address_base, struct.pack("<i", _f80_trunc_i32(x87_stack.pop()) & 0xFFFFFFFF))
                    return 2
            if 0xD8 <= opcode <= 0xDF:
                raise ValueError("x87 instruction at %#x not covered by exact emulation" % address)
            return None

        def hook_code(uc, address, size, user):
            first = bytes(uc.mem_read(address, 1))[0]
            if x87_exact and 0xD8 <= first <= 0xDF:
                try:
                    length = step_x87(uc, address)
                except Exception as error:  # noqa: BLE001
                    uc.emu_stop()
                    state["stopped"] = "error"
                    state["error"] = str(error)[:400]
                    return
                if length is None:
                    uc.emu_stop()
                    state["stopped"] = "error"
                    state["error"] = "x87 instruction at %#x not covered by exact emulation" % address
                    return
                state["instructions"] += 1
                next_pc = address + length
                uc.reg_write(pc_register, next_pc)
                if until is not None and next_pc == until:
                    uc.emu_stop()
                    state["stopped"] = "until"
                elif state["instructions"] >= count:
                    uc.emu_stop()
                    state["stopped"] = "count"
                return
            state["instructions"] += 1
            if until is not None and address == until:
                uc.emu_stop()
                state["stopped"] = "until"
            elif state["instructions"] >= count:
                uc.emu_stop()
                state["stopped"] = "count"

        mu.hook_add(UC_HOOK_CODE, hook_code)
        watchdog = threading.Timer(timeout_secs, mu.emu_stop)
        watchdog.start()
        started = time.monotonic()
        try:
            mu.emu_start(start, stack_top)
        except Exception as error:  # noqa: BLE001 - report instruction errors cleanly
            state["error"] = "%s: %s" % (type(error).__name__, str(error)[:400])
            if state["stopped"] is None:
                state["stopped"] = "error"
        finally:
            watchdog.cancel()
        if state["stopped"] is None:
            state["stopped"] = "exited"
        names = X86_64_REGISTERS if bits == 64 else X86_32_REGISTERS
        registers = {}
        for name in names:
            constant = getattr(__import__("unicorn.x86_const", fromlist=[name]), "UC_X86_REG_" + name.upper())
            registers[name] = mu.reg_read(constant)
        dumps = []
        for dump in params.get("dump") or []:
            address = parse_address(dump["address"])
            size = bounded_int(dump.get("size"), 64, 1, 1_048_576)
            dumps.append({"address": address, "size": size,
                          "data_base64": base64.b64encode(bytes(mu.mem_read(address, size))).decode()})
        return {"bits": bits, "start": start, "stopped": state["stopped"],
                "instructions": state["instructions"], "error": state["error"],
                "duration_secs": round(time.monotonic() - started, 4),
                "registers": registers, "dumps": dumps}
