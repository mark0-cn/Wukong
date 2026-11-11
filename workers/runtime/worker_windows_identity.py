"""Read the kernel System process creation epoch without PowerShell or WMI.

The System process lives for the kernel boot; its creation timestamp survives
worker restarts and is not reconstructed from the changing wall clock/uptime.
Native layouts are bounded and checked; an unsupported query fails closed.
"""
import ctypes


class _ProcessPrefix(ctypes.Structure):
    _fields_ = [
        ("next_offset", ctypes.c_uint32), ("threads", ctypes.c_uint32),
        ("reserved_before_create", ctypes.c_byte * 24),
        ("created", ctypes.c_int64), ("reserved_times", ctypes.c_byte * 16),
        ("name_length", ctypes.c_uint16), ("name_maximum", ctypes.c_uint16),
        ("name_buffer", ctypes.c_void_p), ("priority", ctypes.c_int32),
        ("pid", ctypes.c_size_t),
    ]


def _process_boot_identity(data):
    size = ctypes.sizeof(_ProcessPrefix)
    offset = 0
    for _ in range(65536):
        if offset + size > len(data):
            raise ValueError("truncated native Windows process inventory")
        record = _ProcessPrefix.from_buffer_copy(data, offset)
        if record.next_offset and (record.next_offset < size or offset + record.next_offset > len(data)):
            raise ValueError("invalid native Windows process record extent")
        if record.pid == 4:
            if record.created <= 0:
                raise ValueError("native Windows System creation epoch is unavailable")
            return "windows:" + str(record.created)
        if record.next_offset == 0:
            break
        offset += record.next_offset
    raise ValueError("native Windows System process is absent from the bounded inventory")


def windows_boot_identity():
    # Runtime dynamic linking follows Microsoft's NtQuerySystemInformation
    # contract. Fixed-width fields match the SYSTEM_PROCESS_INFORMATION prefix;
    # no returned pointer (including the image-name pointer) is dereferenced.
    try:
        query = ctypes.WinDLL("ntdll.dll", winmode=0x800).NtQuerySystemInformation
    except (OSError, AttributeError) as error:
        raise ValueError("native Windows boot identity query is unavailable") from error
    query.argtypes = [ctypes.c_uint32, ctypes.c_void_p, ctypes.c_uint32,
                      ctypes.POINTER(ctypes.c_uint32)]
    query.restype = ctypes.c_int32
    size = 65536
    maximum = 16 * 1024 * 1024
    for _ in range(8):
        if size > maximum:
            break
        buffer = ctypes.create_string_buffer(size)
        returned = ctypes.c_uint32()
        status = query(5, buffer, size, ctypes.byref(returned))  # SystemProcessInformation
        code = status & 0xffffffff
        if status >= 0:
            if not ctypes.sizeof(_ProcessPrefix) <= returned.value <= size:
                raise ValueError("invalid native Windows process inventory length")
            return _process_boot_identity(buffer.raw[:returned.value])
        if code not in {0xC0000004, 0xC0000023, 0x80000005}:
            raise ValueError(f"native Windows boot identity query failed (NTSTATUS 0x{code:08x})")
        size = max(size * 2, returned.value + 65536)
    raise ValueError("native Windows process inventory exceeds the boot identity query budget")
