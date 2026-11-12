"""Inert PE/minidump/MSF builders for native library and SDK boundary tests.

These are deliberately small malformed/incomplete binary fixtures, never a
production parser. Genuine compiler PDBs and process dumps cover SDK positives.
"""
import struct
import uuid

GUID = uuid.UUID("12345678-9abc-def0-1234-56789abcdef0")


def cv(guid=GUID, age=7, name=r"Z:\producer-only\probe.pdb"):
    return b"RSDS" + guid.bytes_le + struct.pack("<I", age) + name.encode() + b"\0"


def pe(machine=0x8664, record=None):
    data = bytearray(2048)
    data[:2] = b"MZ"
    struct.pack_into("<I", data, 60, 128)
    data[128:132] = b"PE\0\0"
    optional_size, magic, dirs = (240, 0x20b, 112) if machine == 0x8664 else (224, 0x10b, 96)
    struct.pack_into("<HHIIIHH", data, 132, machine, 1, 123456, 0, 0, optional_size, 2)
    optional = 152
    struct.pack_into("<H", data, optional, magic)
    struct.pack_into("<III", data, optional + 56, 12288, 512, 123)
    struct.pack_into("<I", data, optional + dirs - 4, 16)
    struct.pack_into("<II", data, optional + dirs + 48, 0x1000, 28)
    section = optional + optional_size
    data[section:section + 8] = b".rdata\0\0"
    struct.pack_into("<IIII", data, section + 8, 1024, 0x1000, 1024, 512)
    record = cv() if record is None else record
    struct.pack_into("<IIII", data, 512 + 12, 2, len(record), 0x1040, 576)
    data[576:576 + len(record)] = record
    return data


def dump(records):
    # MINIDUMP_HEADER, one directory and packed 108-byte MINIDUMP_MODULEs.
    data = bytearray(48 + 4 + 108 * len(records))
    data[:4] = b"MDMP"
    struct.pack_into("<III", data, 4, 0xa793, 1, 32)
    struct.pack_into("<III", data, 32, 4, 4 + 108 * len(records), 48)
    struct.pack_into("<I", data, 48, len(records))
    for i, row in enumerate(records):
        name = row.get("name", r"C:\old-location\probe.exe").encode("utf-16-le")
        name_rva = len(data)
        data += struct.pack("<I", len(name)) + name
        record = row.get("cv", cv())
        cv_rva = len(data)
        data += record
        start = 52 + 108 * i
        struct.pack_into("<QIIII", data, start, row.get("base", 0x140000000),
                         row.get("size", 12288), row.get("checksum", 123), row.get("timestamp", 123456), name_rva)
        struct.pack_into("<II", data, start + 76, len(record), cv_rva)
    return data


def pdb(guid=GUID, age=7, block=512):
    data = bytearray(block * 7)
    data[:32] = b"Microsoft C/C++ MSF 7.00\r\n\x1aDS\0\0\0"
    struct.pack_into("<IIIIII", data, 32, block, 1, 7, 16, 0, 3)
    struct.pack_into("<I", data, block * 3, 5)
    # Two streams; empty stream0, stream1 located before its directory block.
    struct.pack_into("<IIII", data, block * 5, 2, 0, 28, 4)
    struct.pack_into("<III", data, block * 4, 20000404, 0, age)
    data[block * 4 + 12:block * 4 + 28] = guid.bytes_le
    return data
