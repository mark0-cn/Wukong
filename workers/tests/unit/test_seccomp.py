"""Real filter bytes, conservative decoding, and a kernel comparison on Linux."""
import base64
import platform
import struct
import subprocess
import sys
import unittest
from wukong_analysis.seccomp import decode, probe


def packed(rows, endian="little"):
    return b"".join(struct.pack(("<" if endian == "little" else ">")+"HBBI", *row) for row in rows)


class SeccompTests(unittest.TestCase):
    def evaluate(self, rows, query, endian="little"):
        program, decoded = decode(packed(rows, endian), endian)
        return probe(program, decoded, query, endian)

    def test_architecture_syscall_and_argument_conditions(self):
        # x86_64 write(fd=1) only; reject the x32 bit and all other calls.
        rows = [(0x20,0,0,4),(0x15,1,0,0xc000003e),(6,0,0,0x80000000),
                (0x20,0,0,0),(0x15,0,3,1),(0x20,0,0,16),(0x15,0,1,1),
                (6,0,0,0x7fff0000),(6,0,0,0x5000d)]
        for arch, nr, fd, expected in [(0xc000003e,1,1,"ALLOW"),(0xc000003e,1,2,"ERRNO"),
                                       (0xc000003e,0x40000001,1,"ERRNO"),(0x40000003,1,1,"KILL_PROCESS")]:
            result = self.evaluate(rows, {"arch":arch,"nr":nr,"args":[fd]})
            self.assertEqual(result["decision"]["action"], expected)
        self.assertEqual(self.evaluate(rows, {"arch":0xc000003e,"nr":1,"args":[2]})["decision"]["data"],13)

    def test_word_halves_follow_byte_order(self):
        for endian, offset in [("little",16),("big",20)]:
            rows = [(0x20,0,0,offset),(0x15,0,1,0x12345678),(6,0,0,0x7fff0000),(6,0,0,0)]
            result = self.evaluate(rows,{"arch":0,"nr":0,"args":[0xaabbccdd12345678]},endian)
            self.assertEqual(result["decision"]["action"],"ALLOW")
            self.assertEqual(decode(packed(rows,endian),endian)[1][0]["operation"],"A = args[0].low32")

    def test_arithmetic_registers_scratch_and_unsigned_wrap(self):
        rows = [(0,0,0,0xffffffff),(4,0,0,2),(2,0,0,0),(1,0,0,1),
                (0x60,0,0,0),(0x1d,0,1,0),(6,0,0,0x7fff0000),(6,0,0,0)]
        self.assertEqual(self.evaluate(rows,{"arch":0,"nr":0})["decision"]["action"],"ALLOW")

    def test_unsupported_or_uninitialized_never_becomes_allow(self):
        for rows in [[(0x28,0,0,0),(6,0,0,0x7fff0000)],[(0x60,0,0,0),(0x16,0,0,0)]]:
            self.assertEqual(self.evaluate(rows,{"arch":0,"nr":0})["status"],"unknown")

    def test_malformed_filters_are_rejected(self):
        for data in [b"",b"123",packed([(5,0,0,3),(6,0,0,0)]),packed([(0x20,0,0,65),(6,0,0,0)]),
                     packed([(0x34,0,0,0),(6,0,0,0)]),b"\0"*32776]:
            with self.assertRaises(ValueError): decode(data)

    @unittest.skipUnless(sys.platform.startswith("linux") and platform.machine()=="x86_64", "Linux x86_64 kernel required")
    def test_decision_matches_installed_kernel_filter(self):
        rows = [(0x20,0,0,0),(0x15,0,1,39),(6,0,0,0x5000d),(6,0,0,0x7fff0000)]
        script = '''import ctypes,base64,sys
class Insn(ctypes.Structure): _fields_=[("code",ctypes.c_ushort),("jt",ctypes.c_ubyte),("jf",ctypes.c_ubyte),("k",ctypes.c_uint)]
class Prog(ctypes.Structure): _fields_=[("len",ctypes.c_ushort),("filter",ctypes.POINTER(Insn))]
raw=base64.b64decode(sys.argv[1]); insns=(Insn*(len(raw)//8)).from_buffer_copy(raw); p=Prog(len(insns),insns)
libc=ctypes.CDLL(None,use_errno=True)
assert libc.prctl(38,1,0,0,0)==0
assert libc.prctl(22,2,ctypes.byref(p),0,0)==0
assert libc.syscall(39)==-1 and ctypes.get_errno()==13
print("kernel_errno_13")
'''
        result = subprocess.run([sys.executable,"-c",script,base64.b64encode(packed(rows)).decode()],capture_output=True,timeout=10)
        self.assertEqual(result.returncode,0,result.stderr.decode())
        self.assertIn(b"kernel_errno_13",result.stdout)
        self.assertEqual(self.evaluate(rows,{"arch":0xc000003e,"nr":39})["decision"]["data"],13)

if __name__ == "__main__": unittest.main()
