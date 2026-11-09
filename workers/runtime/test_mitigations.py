"""Functional protection parsing checks, without an exploit benchmark."""
import struct
import importlib.util
import tempfile
import unittest
from pathlib import Path
from worker_mitigations import gnu_property_features, parse_status, thread_observations


class MitigationTests(unittest.TestCase):
    def test_gnu_features_both_widths_and_byte_orders(self):
        for endian in ["<",">"]:
            for bits in [32,64]:
                prop=struct.pack(endian+"III",0xc0000002,4,3)
                if bits==64: prop+=b"\0"*4
                note=struct.pack(endian+"III",4,len(prop),5)+b"GNU\0"+prop
                self.assertEqual(gnu_property_features(note,endian=="<",bits),[3])
                with self.assertRaises(ValueError): gnu_property_features(note[:-2],endian=="<",bits)

    @unittest.skipUnless(importlib.util.find_spec("elftools"), "optional pyelftools dependency")
    def test_real_elf_parser_preserves_declaration_without_enabling_runtime(self):
        from worker_profile import file_identity
        prop=struct.pack("<IIII",0xc0000002,4,3,0)
        note=struct.pack("<III",4,len(prop),5)+b"GNU\0"+prop
        ident=b"\x7fELF\x02\x01\x01"+b"\0"*9
        header=struct.pack("<16sHHIQQQIHHHHHH",ident,2,62,1,0,64,0,0,64,56,1,64,0,0)
        phdr=struct.pack("<IIQQQQQQ",4,4,120,0,0,len(note),len(note),8)
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/"note.elf"; path.write_bytes(header+phdr+note)
            result=file_identity(path)
            self.assertNotIn("elf_metadata_error",result)
            self.assertTrue(result["elf_properties"]["shstk"])
            self.assertTrue(result["elf_properties"]["ibt"])
            self.assertEqual(result["elf_properties"]["runtime_enabled"],"unknown")

    def test_non_feature_notes_are_not_cet_declarations(self):
        note=struct.pack("<III",4,4,3)+b"GNU\0"+b"1234"
        self.assertEqual(gnu_property_features(note,True,64),[])

    def test_missing_status_is_unknown_not_disabled(self):
        self.assertIsNone(parse_status("Name:	target\n")["shstk_enabled"])
        self.assertFalse(parse_status("x86_Thread_features:	\n")["shstk_enabled"])

    def test_seccomp_mode_does_not_invent_policy(self):
        result=parse_status("Seccomp:	2\nSeccomp_filters:	1\nx86_Thread_features:	shstk wrss\n")
        self.assertEqual(result["seccomp_mode"],2)
        self.assertEqual(result["seccomp_policy"],"unknown")
        self.assertTrue(result["shstk_enabled"])
        self.assertIsNone(result["ibt_enabled"])

    def test_thread_differences_remain_visible(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)
            for tid,value in [(1,"shstk"),(2,"")]:
                path=root/"task"/str(tid); path.mkdir(parents=True)
                (path/"status").write_text("x86_Thread_features:	"+value+"\n")
            result=thread_observations(root)
            states={item["tid"]:item["values"]["shstk_enabled"] for item in result["threads"]}
            self.assertEqual(states,{1:True,2:False})
            self.assertTrue(result["complete"])

if __name__ == "__main__": unittest.main()
