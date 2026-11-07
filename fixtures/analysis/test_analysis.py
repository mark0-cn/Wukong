"""Validate scoring boundaries; fabricated reports here are test fixtures only."""
import copy
import hashlib
from pathlib import Path
import tempfile
import unittest

from score import score


class ScoreTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        target = Path(self.temp.name) / "sample"
        target.write_bytes(b"trusted scoring fixture")
        digest = hashlib.sha256(target.read_bytes()).hexdigest()
        self.tasks = {"cases": [
            {"id": "format", "binary": "sample", "questions": ["layout", "control"]},
            {"id": "algorithm", "binary": "sample", "questions": ["arithmetic"]},
        ]}
        self.build = {"targets": {"sample": {"path": str(target), "sha256": digest}}}
        self.reports = {"format": {"snapshot": {"run": {"id": "run-one", "target_sha256": digest,
                                                                   "total_tokens": 17, "cost_usd": 0.0},
                                              "evidence": [{"id": "observation"}], "findings": []}}}
        self.labels = [
            {"case": "format", "question_id": "layout", "verdict": "correct",
             "rationale": "Independently checked against the parser", "evidence_ids": ["observation"]},
            {"case": "format", "question_id": "control", "verdict": "unresolved",
             "rationale": "Control experiment was not run", "evidence_ids": []},
        ]

    def tearDown(self):
        self.temp.cleanup()

    def test_general_answer_needs_no_vulnerability_and_missing_tasks_remain_unresolved(self):
        result = score(self.tasks, self.build, self.reports, self.labels)
        self.assertEqual(result["correct"], 1)
        self.assertEqual(result["unresolved"], 2)
        self.assertEqual(result["missing_cases"], ["algorithm"])
        self.assertEqual(result["model_total_tokens"], 17)

    def test_correct_verdict_requires_original_report_evidence(self):
        for links in [[], ["invented-evidence"]]:
            labels = copy.deepcopy(self.labels)
            labels[0]["evidence_ids"] = links
            with self.assertRaisesRegex(ValueError, "linked report evidence"):
                score(self.tasks, self.build, self.reports, labels)

    def test_unreviewed_and_duplicate_answers_cannot_be_scored(self):
        for labels in [self.labels[:1], self.labels + self.labels[:1]]:
            with self.assertRaises(ValueError):
                score(self.tasks, self.build, self.reports, labels)

    def test_target_identity_is_checked(self):
        reports = copy.deepcopy(self.reports)
        reports["format"]["snapshot"]["run"]["target_sha256"] = "wrong"
        with self.assertRaisesRegex(ValueError, "SHA256 mismatch"):
            score(self.tasks, self.build, reports, self.labels)


if __name__ == "__main__":
    unittest.main()
