import unittest
from scripts.ci.cleanup_ci_cache import candidates


class CleanupTests(unittest.TestCase):
    def test_preserves_master_tags_live_prs_and_unknown_branches(self):
        refs = ["refs/heads/master", "refs/tags/v1.2.3", "refs/pull/1/merge", "refs/pull/2/merge",
                "refs/heads/dependabot/old", "refs/heads/dependabot/live", "refs/heads/topic", "refs/pull/99/merge"]
        caches = [dict(id=i, ref=ref) for i, ref in enumerate(refs)]
        def pr(number, head, state):
            return dict(number=number, state=state, user=dict(login="dependabot[bot]"),
                        head=dict(ref=head, repo=dict(full_name="o/r")))
        closed = [pr(1, "dependabot/old", "closed"), pr(2, "dependabot/live", "closed")]
        opened = [pr(2, "dependabot/live", "open")]
        self.assertEqual([c["id"] for c in candidates(caches, closed, opened, "o/r")], [2, 4])
        closed[0]["head"]["repo"]["full_name"] = "fork/r"
        self.assertEqual([c["id"] for c in candidates(caches, closed, opened, "o/r")], [2])
