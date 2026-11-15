import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from wukong_analysis.validator import evaluate, scenarios
from wukong_worker.execution.validator import invoke, run
from validator_fixtures import candidate


class ValidatorTests(unittest.TestCase):
    def exercise(self, shell=False, broken=False):
        with tempfile.TemporaryDirectory() as directory:
            script = Path(directory) / ("script.sh" if shell else "script.py")
            script.write_text(candidate(broken, shell), encoding="utf-8")
            interpreter = shutil.which("bash") if shell else sys.executable
            subprocess.run(
                [interpreter, str(script), "--verify"],
                cwd=directory,
                check=True,
                capture_output=True,
            )
            baseline = Path(directory) / "baseline.json"
            return run(interpreter, str(script), str(baseline), ["--verify"])

    def test_standalone_python_template_and_every_perturbation(self):
        report = self.exercise()
        self.assertTrue(report["passed"], report)
        self.assertGreater(len(report["cases"]), 12)
        # The real negative control exits 7; normal baseline still passes.
        self.assertEqual(report["cases"][0]["observed"]["exit_code"], 0)

    @unittest.skipUnless(
        sys.platform.startswith("linux") and shutil.which("bash"), "POSIX Shell real execution"
    )
    def test_standalone_shell_uses_the_same_protocol(self):
        self.assertTrue(self.exercise(shell=True)["passed"])

    def test_old_banner_only_predicate_is_rejected(self):
        report = self.exercise(broken=True)
        self.assertFalse(report["passed"])
        self.assertTrue(
            any(
                not x["passed"] and x["scenario"].endswith("signal_after_output")
                for x in report["cases"]
            )
        )

    def test_missing_and_empty_manifest_cannot_pass(self):
        for manifest in [
            [],
            [{"id": "x", "kind": "unknown"}],
            [{"id": "x", "kind": "comparison", "expected": 1}] * 2,
        ]:
            with self.assertRaises((ValueError, KeyError)):
                evaluate(manifest, {"x": {}})
        self.assertEqual(
            evaluate([{"id": "x", "kind": "comparison", "expected": 1}], {}), "inconclusive"
        )

    def test_protocol_output_is_bounded_and_timeout_is_not_pass(self):
        output = invoke([sys.executable, "-c", "print('x'*100000)"], {})
        self.assertIn("error", output)
        timeout = invoke([sys.executable, "-c", "import time; time.sleep(10)"], {})
        self.assertIn("error", timeout)

    def test_inconsistent_baseline_is_not_an_acceptance_seed(self):
        self.assertEqual(
            evaluate(
                [{"id": "count", "kind": "comparison", "expected": 1}],
                {"count": {"observed": True}},
            ),
            "fail",
        )
        with self.assertRaises(ValueError):
            scenarios(
                [{"id": "map", "kind": "comparison", "expected": 1}], {"map": {"observed": 0}}
            )


if __name__ == "__main__":
    unittest.main()
