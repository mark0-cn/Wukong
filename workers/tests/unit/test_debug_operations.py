"""Typed operations reject command injection before reaching either native engine."""
import unittest

from worker_debug_operations import control_request, inspect_request


class TypedDebugRequests(unittest.TestCase):
    def test_symbol_cannot_embed_debugger_commands(self):
        for symbol in ['main; quit', 'main\n-shell', 'foo!bar .shell cmd', 'foo`cmd`', '$(quit)', 'a\0b', '']:
            with self.subTest(symbol=symbol), self.assertRaises(ValueError):
                control_request({'action':'breakpoint_add','symbol':symbol})
        self.assertEqual(control_request({'action':'breakpoint_add','symbol':'fixture!function'})['symbol'],'fixture!function')

    def test_memory_range_cannot_overflow_or_exceed_reply_bound(self):
        for params in [dict(address=2**64-1,length=2),dict(address=-1),dict(address=True),
                       dict(address=0,length=65537),dict(address=0,length=0),dict(address='a;quit')]:
            with self.subTest(params=params), self.assertRaises(ValueError):
                inspect_request({'kind':'memory',**params})
        self.assertEqual(inspect_request({'kind':'memory','address':'0x1000','length':32})['address'],4096)

    def test_breakpoint_identity_and_location_are_unambiguous(self):
        for params in [dict(action='breakpoint_add'),dict(action='breakpoint_add',address=1,symbol='main'),
                       dict(action='breakpoint_remove',breakpoint_id='1 -exec-run'),dict(action='shell')]:
            with self.subTest(params=params), self.assertRaises(ValueError):
                control_request(params)

    def test_inspection_kind_and_row_limit_are_bounded(self):
        for params in [dict(kind='execute'),dict(kind='registers',limit=0),dict(kind='stack',limit=257)]:
            with self.subTest(params=params), self.assertRaises(ValueError):
                inspect_request(params)
