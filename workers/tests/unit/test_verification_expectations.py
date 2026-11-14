import unittest

from worker_job_supervisor import expectation_matches


class VerificationExpectationsTests(unittest.TestCase):
    def test_null_optional_predicates_do_not_reject_successful_behavior(self):
        result = dict(state="completed", exit_code=0, signal=None, stdout="value=7\n", stderr="")
        expected = dict(exit_code=0, signal=None, stdout_contains=None, stderr_contains=None)
        self.assertTrue(expectation_matches(result, expected))
        self.assertFalse(expectation_matches(dict(result, exit_code=1), expected))
        self.assertFalse(expectation_matches(dict(result, state="timed_out"), expected))
        self.assertFalse(expectation_matches(result, dict(expected, stdout_contains="value=8")))

    def test_empty_expectations_still_require_a_crash(self):
        result = dict(state="completed", exit_code=0, signal=None, stdout="", stderr="")
        for expected in [None, {}, dict(exit_code=None, signal=None, stdout_contains=None, stderr_contains=None)]:
            with self.subTest(expected=expected):
                self.assertFalse(expectation_matches(result, expected))
                self.assertTrue(expectation_matches(dict(result, exit_code=-11, signal=11), expected))
        with self.assertRaises(ValueError):
            expectation_matches(result, {"unknown": None})
