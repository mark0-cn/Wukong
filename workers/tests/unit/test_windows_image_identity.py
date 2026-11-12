"""Portable correspondence policy; native format checks use real library/SDK calls."""
import copy
from pathlib import Path
import tempfile
import unittest

from worker_common import sha256_file
from worker_windows_image_identity import match_module, local_symbols

GUID = "123456789abcdef0123456789abcdef0"


def image():
    return {"machine":0x8664, "architecture":"x64", "size":12288, "timestamp":123456,
            "checksum":123, "codeview":{"format":"RSDS", "guid":GUID, "age":7, "pdb_name":"probe.pdb"}}


def module(**changes):
    row = {**image(), "base":0x140000000, "image":r"C:\old-location\probe.exe"}
    row.update(changes)
    return row


class WindowsIdentityTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)

    def test_renamed_staged_image_matches_metadata_and_rsds_without_claiming_byte_identity(self):
        matched = match_module(image(), [module()], "renamed.exe", True)
        self.assertEqual(matched["scope"], "module_metadata_and_rsds")
        self.assertTrue(matched["checksum_compared"])
        self.assertTrue(matched["codeview_matched"])
        self.assertFalse(matched["byte_identity_verified"])

    def test_wrong_module_size_timestamp_checksum_guid_or_age_is_rejected(self):
        for changes in [{"size":8192}, {"timestamp":123457}, {"checksum":124},
                        {"codeview":{**image()["codeview"], "guid":"1" * 32}},
                        {"codeview":{**image()["codeview"], "age":8}}]:
            with self.subTest(changes=changes), self.assertRaisesRegex(ValueError, "does not uniquely correspond"):
                match_module(image(), [module(**changes)], "probe.exe", True)

    def test_missing_codeview_is_explicit_and_duplicates_do_not_establish_identity(self):
        row = module(codeview=None, checksum=0)
        result = match_module(image(), [row], "probe.exe", True)
        self.assertEqual(result["scope"], "module_metadata_only")
        self.assertFalse(result["codeview_matched"])
        self.assertFalse(result["checksum_compared"])
        with self.assertRaises(ValueError):
            match_module(image(), [row, row], "probe.exe", True)

    def test_missing_image_codeview_is_an_explicit_gap(self):
        result = match_module({**image(), "codeview":None}, [module()], "probe.exe", True)
        self.assertFalse(result["codeview_matched"])
        self.assertEqual(result["scope"], "module_metadata_only")

    def test_named_conflict_cannot_be_hidden_by_another_similar_module(self):
        with self.assertRaises(ValueError):
            match_module(image(), [module(timestamp=123457), module(image="other.dll", base=0x180000000)], "probe.exe", True)

    def test_local_symbols_do_not_follow_embedded_paths_or_accept_wrong_guid_age(self):
        symbols = self.root / "symbols"
        symbols.mkdir()
        (self.root / "probe.pdb").write_bytes(b"outside configured cache")
        def unused(_):
            self.fail("missing/unsafe symbol was opened")
        self.assertEqual(local_symbols(image(), symbols, unused, unused)["status"], "missing")
        candidate = symbols / "probe.pdb"
        candidate.write_bytes(b"inert fixture; native SDK observation is supplied")
        observation = {"guid":GUID, "age":7, "file_sha256":sha256_file(candidate), "source":"test_observation"}
        for wrong in [{**observation, "guid":"1" * 32}, {**observation, "age":8}]:
            with self.assertRaisesRegex(ValueError, "GUID/age"):
                local_symbols(image(), symbols, lambda path:path, lambda path:wrong)
        result = local_symbols(image(), symbols, lambda path:path, lambda path:observation)
        self.assertTrue(result["pdb_identity_matched"])
        self.assertEqual(result["pdb_sha256"], sha256_file(candidate))
        for name in ["NUL.pdb", "COM1.pdb", "LPT¹.pdb", "x:stream.pdb", "probe.pdb ", "../probe.pdb"]:
            invalid = copy.deepcopy(image())
            invalid["codeview"]["pdb_name"] = name
            self.assertEqual(local_symbols(invalid, symbols, unused, unused)["status"], "unusable_pdb_name")

    def test_pdb_content_change_cannot_inherit_a_previous_sdk_identity(self):
        candidate = self.root / "probe.pdb"
        candidate.write_bytes(b"different bytes")
        with self.assertRaisesRegex(ValueError, "changed"):
            local_symbols(image(), self.root, lambda path:path, lambda path:{"guid":GUID, "age":7, "file_sha256":"0" * 64})

    def test_unsupported_identity_and_empty_module_observations_are_explicit(self):
        self.assertEqual(local_symbols({**image(), "codeview":None}, self.root, None, None)["status"], "no_supported_rsds_identity")
        for rows in [[], {}, [module()] * 4097]:
            with self.assertRaisesRegex(ValueError, "bounded"):
                match_module(image(), rows, "probe.exe", True)
