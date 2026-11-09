"""Bounded Unicorn emulation for VM and hand-crafted ELF analysis.

`emu.run` executes the target from a start address under the Unicorn
emulator, mapping the ELF's load segments as writable memory plus a fixed
stack, and returns registers and memory dumps. It is a pure simulation: the
target never runs natively and no worker state is touched. Instruction
count, a stop address and a wall-clock watchdog bound the run.
"""
import base64
import json
import threading
import time

from worker_common import bounded_int, confined, parse_address, WorkerError

X86_64_REGISTERS = ["rax", "rbx", "rcx", "rdx", "rsi", "rdi", "rbp", "rsp", "rip",
                    "r8", "r9", "r10", "r11", "r12", "r13", "r14", "r15", "eflags"]
X86_32_REGISTERS = ["eax", "ebx", "ecx", "edx", "esi", "edi", "ebp", "esp", "eip", "eflags"]


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
        for name, value in init.items():
            if not isinstance(value, int):
                raise ValueError("init_regs values must be integers")
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
            if name not in table:
                raise ValueError("unknown register for this mode: " + name)
            mu.reg_write(table[name], value)
        for write in params.get("memory") or []:
            mu.mem_write(parse_address(write["address"]),
                         base64.b64decode(write["data_base64"], validate=True))
        state = {"instructions": 0, "stopped": None, "error": None}

        def hook_code(uc, address, size, user):
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
