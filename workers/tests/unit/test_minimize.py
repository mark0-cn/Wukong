"""Reduction integration invariants; Picire's algorithm is not reimplemented."""
import time
import unittest
import tempfile
from pathlib import Path
from unittest.mock import patch

from worker_minimize import reduce_input


class MinimizeTests(unittest.TestCase):
    def run_reduce(self, data, oracle, maximum=128, cancelled=lambda: False):
        calls = []
        def evaluate(candidate, index):
            calls.append((candidate, index))
            return oracle(candidate)
        result, metadata = reduce_input(data, evaluate, max_attempts=maximum,
            deadline=time.monotonic() + 10, cancelled=cancelled)
        self.assertEqual(metadata["attempts"], len(calls) + 1)
        self.assertEqual([index for _, index in calls], list(range(1, len(calls) + 1)))
        self.assertTrue(oracle(result))
        return result, metadata, calls

    def test_preserves_order_and_binary_bytes_and_checks_empty(self):
        data, result, calls = self.run_reduce(b"x\xffAB\0x", lambda b: b"\xff" in b and b"\0" in b)
        self.assertEqual(data, b"\xff\0")
        self.assertTrue(result["complete"])
        self.assertEqual(calls[0][0], b"")

    def test_empty_reproducer_is_retained(self):
        data, result, _ = self.run_reduce(b"padding", lambda _: True)
        self.assertEqual(data, b"")
        self.assertTrue(result["complete"])
        self.assertEqual(result["attempts"], 2)

    def test_baseline_consumes_budget_and_is_not_reexecuted(self):
        data, result, calls = self.run_reduce(b"AAA", lambda b: b"A" in b, maximum=1)
        self.assertEqual(data, b"AAA")
        self.assertFalse(result["complete"])
        self.assertEqual(calls, [])

    def test_cancellation_retains_only_verified_candidate(self):
        cancelled = [False]
        def oracle(candidate):
            cancelled[0] = True
            return b"A" in candidate
        data, result = reduce_input(b"xxAyy", lambda data, _: oracle(data),
            max_attempts=128, deadline=time.monotonic() + 10, cancelled=lambda: cancelled[0])
        self.assertEqual(data, b"xxAyy")
        self.assertFalse(result["complete"])
        self.assertEqual(result["attempts"], 2)
        self.assertEqual(result["stop_reason"], "cancelled")

    def test_expired_deadline_does_not_execute_a_candidate(self):
        with patch("worker_minimize.time.monotonic", return_value=10):
            data, result = reduce_input(b"A", lambda *_: self.fail("executed after deadline"),
                max_attempts=128, deadline=10, cancelled=lambda: False)
        self.assertEqual(data, b"A")
        self.assertFalse(result["complete"])
        self.assertEqual(result["attempts"], 1)

    def test_unresolved_candidate_cannot_prove_minimality(self):
        data, result = reduce_input(b"A", lambda *_: None, max_attempts=128,
            deadline=time.monotonic() + 10, cancelled=lambda: False)
        self.assertEqual(data, b"A")
        self.assertFalse(result["complete"])
        self.assertEqual(result["stop_reason"], "candidate outcome unresolved")

    def test_missing_reducer_dependency_hides_both_minimization_capabilities(self):
        from worker_capabilities import doctor
        with tempfile.TemporaryDirectory() as root, \
                patch("worker_capabilities.shutil.which", return_value=None), \
                patch("worker_capabilities._ghidra_ready", return_value=False):
            for absent in ("picire", "chardet", "inators"):
                with self.subTest(absent=absent), patch("worker_capabilities._python_module", side_effect=lambda n: n != absent):
                    result = doctor(Path(root), True, False)
                    self.assertFalse(result["python_toolchain"]["picire"])
                    self.assertFalse(result["capabilities"]["input_minimize"])
                    self.assertFalse(result["capabilities"]["input_minimize_signal_only"])


if __name__ == "__main__":
    unittest.main()
