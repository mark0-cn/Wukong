"""Bounded PE dependency inspection for offline Windows bootstrap binaries."""
from pathlib import Path
import re
import struct


def imported_libraries(path, architecture="x64"):
    with Path(path).open("rb") as stream:
        data = stream.read(64 * 1024 * 1024 + 1)
    if len(data) > 64 * 1024 * 1024:
        raise ValueError("Bootstrap PE image exceeds 64 MiB")

    def unpack(format_, offset):
        size = struct.calcsize(format_)
        if offset < 0 or offset + size > len(data):
            raise ValueError("Truncated PE image")
        return struct.unpack_from(format_, data, offset)

    if data[:2] != b"MZ":
        raise ValueError("Expected a PE image")
    pe, = unpack("<I", 60)
    if pe > 1024 * 1024 or data[pe:pe + 4] != b"PE\0\0":
        raise ValueError("Invalid PE header")
    machine, sections = unpack("<HH", pe + 4)
    optional_size, = unpack("<H", pe + 20)
    optional = pe + 24
    magic, = unpack("<H", optional)
    expected = {"x64": (0x8664, 0x20b, 112), "x86": (0x14c, 0x10b, 96)}.get(architecture)
    if expected is None:
        raise ValueError("Unsupported Windows PE architecture")
    if (machine, magic) != expected[:2] or not 1 <= sections <= 96 or optional_size < expected[2] + 16:
        raise ValueError("Expected a Windows " + architecture + " PE image")
    directory_base = optional + expected[2]
    directories, = unpack("<I", directory_base - 4)
    headers_size, = unpack("<I", optional + 60)
    table = optional + optional_size

    def offset(rva, size):
        if rva < headers_size and rva + size <= min(headers_size, len(data)):
            return rva
        for index in range(sections):
            virtual_size, address, raw_size, raw = unpack("<IIII", table + index * 40 + 8)
            if address <= rva < address + max(virtual_size, raw_size):
                delta = rva - address
                if delta + size <= raw_size and raw + delta + size <= len(data):
                    return raw + delta
                raise ValueError("PE import points outside file-backed section")
        raise ValueError("PE import points outside the image")

    def name(rva):
        value = bytearray()
        for index in range(256):
            byte = data[offset(rva + index, 1)]
            if byte == 0:
                result = value.decode("ascii")
                if not re.fullmatch(r"[A-Za-z0-9_-]+\.dll", result, re.IGNORECASE):
                    raise ValueError("Invalid imported library name")
                return result.lower()
            value.append(byte)
        raise ValueError("Unbounded PE library name")

    imports = set()
    # Import directory and delay-import directory. PE32+ delay descriptors must
    # use RVA addressing; rejecting other forms avoids truncating virtual addresses.
    for index, descriptor_size in [(1, 20), (13, 32)]:
        if directories <= index:
            continue
        directory = directory_base + index * 8
        if directory + 8 > optional + optional_size:
            raise ValueError("PE directory exceeds optional header")
        rva, size = unpack("<II", directory)
        if rva == 0 and size == 0:
            continue
        if rva == 0 or not descriptor_size <= size <= 64 * 1024:
            raise ValueError("Invalid PE import directory")
        for position in range(0, size - descriptor_size + 1, descriptor_size):
            record = unpack("<" + "I" * (descriptor_size // 4), offset(rva + position, descriptor_size))
            if not any(record):
                break
            if index == 13 and record[0] != 1:
                raise ValueError("Unsupported PE delay-import addressing")
            imports.add(name(record[3] if index == 1 else record[1]))
        else:
            raise ValueError("Unterminated PE import directory")
    if not imports:
        raise ValueError("Native worker has no inspectable system imports")
    return sorted(imports)


def require_static_crt(path, architecture="x64"):
    imports = imported_libraries(path, architecture)
    forbidden = [name for name in imports if name.startswith(("vcruntime", "msvcp", "msvcr", "ucrtbase", "api-ms-win-crt-"))]
    if forbidden:
        raise ValueError("Offline bootstrap binary requires an external C runtime: " + ", ".join(forbidden))
    return imports
