import copy
import unittest
from compare_runs import compare

class ExperimentTests(unittest.TestCase):
    def fixture(self):
        report = {"snapshot":{"run":{"target_sha256":"abc","total_tokens":100,"cost_usd":0.1},"evidence":[{"id":"e"}],"calls":[],"agents":[]}}
        base = {"case":"format","repeat":0,"condition":"baseline","model":"fixed","budget":1000,"target_sha256":"abc","questions":["length"],"report":report,"reviews":[{"question_id":"length","verdict":"correct","rationale":"checked boundary","evidence_ids":["e"]}]}
        candidate = copy.deepcopy(base); candidate["condition"] = "candidate"
        candidate["report"]["snapshot"]["run"]["total_tokens"] = 50
        return [base, candidate]
    def test_paired_cost_without_inventing_latency(self):
        result = compare(self.fixture())
        self.assertEqual(result["mean_paired_delta"]["tokens"], -50)
        self.assertIsNone(result["mean_paired_delta"]["elapsed_seconds"])
    def test_reject_confounded_and_missing_pairs(self):
        entries = self.fixture(); entries[1]["model"] = "different"
        with self.assertRaises(ValueError): compare(entries)
        with self.assertRaises(ValueError): compare(entries[:1])
    def test_no_credit_for_unlinked_correctness(self):
        entries = self.fixture(); entries[1]["reviews"][0]["evidence_ids"] = []
        with self.assertRaises(ValueError): compare(entries)
    def test_false_claim_acceptance_is_separate_from_tool_success(self):
        entries = self.fixture()
        for entry in entries: entry["reviews"][0].update(injected_false_claim=True, accepted_by_agent=False)
        entries[1]["reviews"][0]["accepted_by_agent"] = True
        self.assertEqual(compare(entries)["mean_paired_delta"]["false_claim_acceptance_rate"], 1)

if __name__ == "__main__": unittest.main()
