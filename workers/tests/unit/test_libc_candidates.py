"""Compare actual ELF DSOs with deliberately ambiguous first-symbol offsets."""
import hashlib
import importlib.util
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tempfile
import unittest
from worker_libc import LibcService


@unittest.skipUnless(sys.platform.startswith("linux") and platform.machine()=="x86_64" and shutil.which("gcc") and importlib.util.find_spec("elftools"), "Linux x86_64 GCC and pyelftools required")
class LibcCandidateTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.service = LibcService(self.root,True,False)
        for name, padding in [("a",16),("b",64)]:
            source = self.root/(name+".S")
            source.write_text('.text\n.globl puts\n.type puts,@function\nputs: ret\n.space %d\n.globl read\n.type read,@function\nread: ret\n.section .rodata\n.asciz "/bin/sh"\n.section .note.GNU-stack,"",@progbits\n' % padding)
            subprocess.run(["gcc","-shared","-nostdlib","-Wl,--build-id","-o",str(self.root/(name+".so")),str(source)],check=True,capture_output=True)
        from elftools.elf.elffile import ELFFile
        with (self.root/"a.so").open("rb") as stream:
            symbols=ELFFile(stream).get_section_by_name(".dynsym")
            self.offsets={s.name:int(s["st_value"]) for s in symbols.iter_symbols()}
        self.base=0x7f1000000000

    def query(self, leaks, **extra):
        return self.service.identify({"leaks":leaks,"candidates":["a.so","b.so"],**extra})

    def test_single_leak_preserves_ambiguity_and_content_identity(self):
        result=self.query([{"address":self.base+self.offsets["puts"],"symbol":"puts"}])
        self.assertTrue(result["ambiguous"])
        self.assertFalse(result["matched"])
        self.assertEqual(len(result["candidates"]),2)
        self.assertEqual(result["candidates"][0]["sha256"],hashlib.sha256((self.root/"a.so").read_bytes()).hexdigest())
        self.assertIsNotNone(result["candidates"][0]["build_id"])

    def test_second_leak_discriminates_and_addend_is_explicit(self):
        result=self.query([{"address":self.base+self.offsets["puts"]+7,"symbol":"puts","addend":7},
                           {"address":self.base+self.offsets["read"],"symbol":"read"}])
        self.assertFalse(result["ambiguous"])
        self.assertEqual(len(result["candidates"]),1)
        self.assertEqual(result["candidates"][0]["base"],self.base)
        self.assertIsNotNone(result["candidates"][0]["binsh_offset"])

    def test_inconsistent_process_leaks_do_not_match(self):
        result=self.query([{"address":self.base+self.offsets["puts"],"symbol":"puts"},
                           {"address":self.base+0x1000+self.offsets["read"],"symbol":"read"}])
        self.assertFalse(result["candidate_match"])

    def test_bad_candidates_missing_symbols_and_path_escape(self):
        (self.root/"bad.so").write_bytes(b"not ELF")
        result=self.query([{"address":self.base,"symbol":"absent"}],candidates=["bad.so","a.so"])
        self.assertFalse(result["candidate_match"])
        self.assertIn("error",result["compared"][0])
        with self.assertRaises(ValueError): self.query([{"address":self.base,"symbol":"puts"}],candidates=["../escape"])

    def test_same_content_is_not_two_independent_candidates(self):
        shutil.copyfile(self.root/"a.so",self.root/"duplicate.so")
        result=self.query([{"address":self.base+self.offsets["puts"],"symbol":"puts"}],candidates=["a.so","duplicate.so"])
        self.assertFalse(result["ambiguous"])
        self.assertEqual(len(result["compared"]),1)

    def test_target_architecture_mismatch_and_legacy_input(self):
        data=bytearray((self.root/"a.so").read_bytes());data[18:20]=(183).to_bytes(2,"little")
        (self.root/"arm.so").write_bytes(data)
        result=self.query([{"address":self.base+self.offsets["puts"],"symbol":"puts"}],target="arm.so")
        self.assertFalse(result["candidate_match"])
        legacy=self.service.identify({"leaked":self.base+self.offsets["puts"],"symbol":"puts","candidates":["a.so"]})
        self.assertTrue(legacy["candidate_match"])
        self.assertFalse(legacy["matched"])

if __name__ == "__main__": unittest.main()
