"""Pwn task manifest selection and honest scoring of unrun tasks."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from evals.analysis.score import score
from evals.pwn.evaluate import ROOT


class PwnEvaluationTests(unittest.TestCase):
    def test_mechanism_tasks_without_runs_have_no_accuracy_credit(self):
        tasks=json.loads((ROOT/"tasks.json").read_text())
        result=score(tasks,{"targets":{}},{},[])
        self.assertEqual(result["correct"],0)
        self.assertEqual(result["unresolved"],sum(len(case["questions"]) for case in tasks["cases"]))
        self.assertFalse(result["complete_suite"])

    def test_cli_selects_manifest_and_rejects_changed_task_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)
            tasks=root/"tasks.json"
            tasks.write_text(json.dumps({"cases":[{"id":"mechanism","binary":"unrun","questions":["observed","limits"]}]}))
            (root/"build.json").write_text(json.dumps({"tasks_sha256":hashlib.sha256(tasks.read_bytes()).hexdigest(),"targets":{}}))
            (root/"reports.json").write_text("{}")
            (root/"reviews.json").write_text('{"labels":[]}')
            command=[sys.executable,"-m","evals.analysis.score","--tasks",str(tasks),"--build-dir",str(root),
                     "--reports",str(root/"reports.json"),"--adjudications",str(root/"reviews.json"),"--output",str(root/"metrics.json")]
            result=subprocess.run(command,capture_output=True,timeout=10)
            self.assertEqual(result.returncode,0,result.stderr.decode())
            self.assertEqual(json.loads((root/"metrics.json").read_text())["unresolved"],2)
            tasks.write_text('{"cases":[]}')
            result=subprocess.run(command,capture_output=True,timeout=10)
            self.assertNotEqual(result.returncode,0)
            self.assertIn(b"manifest changed",result.stderr)

if __name__=="__main__": unittest.main()
