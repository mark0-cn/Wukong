"""Bind the loader boot sequence to its OS boot-entry identity.

The shared BootId increments on each boot attempt. The boot-entry GUID alone
can persist across reboots, so both are required. Neither comes from wall-clock
arithmetic, process enumeration, an external command or a generated fallback.
"""
import ctypes
import struct
import uuid


SHARED_PREFIX_BYTES = 0x2C8


def _decode_boot_identity(environment, shared):
    if len(environment) not in {24, 32} or len(shared) != SHARED_PREFIX_BYTES:
        raise ValueError("unsupported native Windows boot information length")
    major = struct.unpack_from('<I', shared, 0x26C)[0]
    build = struct.unpack_from('<I', shared, 0x260)[0] & 0x0FFFFFFF
    architecture = struct.unpack_from('<H', shared, 0x26A)[0]
    if major != 10 or build < 10240 or architecture not in {0, 9}:
        raise ValueError("unsupported Windows shared boot information layout")
    identifier = uuid.UUID(bytes_le=bytes(environment[:16]))
    firmware = struct.unpack_from('<I', environment, 16)[0]
    if identifier.int == 0 or firmware not in {1, 2}:
        raise ValueError("native Windows boot-entry identity is unavailable")
    sequence = struct.unpack_from('<I', shared, 0x2C4)[0]
    return 'windows:' + identifier.hex + ':' + str(sequence)


def windows_boot_identity():
    # Dynamic linking follows Microsoft's native-query contract. The public
    # read-only KUSER_SHARED_DATA mapping remains fixed in user mode. Read only
    # its bounded version/architecture/BootId prefix; never follow a pointer.
    try:
        query = ctypes.WinDLL('ntdll.dll', winmode=0x800).NtQuerySystemInformation
        shared = ctypes.string_at(0x7FFE0000, SHARED_PREFIX_BYTES)
    except (OSError, AttributeError) as error:
        raise ValueError('native Windows boot identity query is unavailable') from error
    query.argtypes = [ctypes.c_uint32, ctypes.c_void_p, ctypes.c_uint32,
                      ctypes.POINTER(ctypes.c_uint32)]
    query.restype = ctypes.c_int32
    buffer = ctypes.create_string_buffer(32)
    returned = ctypes.c_uint32()
    status = query(90, buffer, 32, ctypes.byref(returned))  # SystemBootEnvironmentInformation
    if status < 0:
        raise ValueError(f'native Windows boot environment query failed (NTSTATUS 0x{status & 0xffffffff:08x})')
    if returned.value not in {24, 32}:
        raise ValueError('unsupported native Windows boot environment length')
    return _decode_boot_identity(buffer.raw[:returned.value], shared)
