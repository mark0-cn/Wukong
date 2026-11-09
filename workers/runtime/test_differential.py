import base64
import sys
import tempfile
import unittest
from pathlib import Path
from differential_runner import compare, observe

class DifferentialTests(unittest.TestCase):
    def test_equal_and_mismatch_preserve_original_input(self):
        vector = base64.b64encode(b"print(123)\n").decode()
        result = compare(sys.executable, sys.executable, [vector], 2)
        self.assertEqual(result["matches"], 1)
        with tempfile.TemporaryDirectory() as directory:
            reference = Path(directory) / "reference.py"
            reference.write_text("print(456)")
            result = compare(sys.executable, reference, [vector], 2, True)
            self.assertEqual(result["mismatches"], 1)
            self.assertEqual(result["cases"][0]["input_base64"], vector)
    def test_timeout_is_not_a_match(self):
        result = observe([sys.executable, "-c", "import time; time.sleep(5)"], b"", 0.1)
        self.assertTrue(result["timeout"])
    def test_limits(self):
        with self.assertRaises(ValueError): compare(sys.executable, sys.executable, [], 2)
        with self.assertRaises(ValueError): compare(sys.executable, sys.executable, ["!"], 2)

if __name__ == "__main__": unittest.main()
