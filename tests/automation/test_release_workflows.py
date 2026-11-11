"""Exercise the publication gate with normal reuse skips and failed dependencies."""
from pathlib import Path
from types import SimpleNamespace as NS
import re
import unittest


class PublishGateTests(unittest.TestCase):
    def allowed(self, event="push", ref="refs/tags/v0.2.0", metadata="success", package="success", upstream_skipped=True):
        workflow = Path(".github/workflows/release.yml").read_text()
        condition = re.search(r"  publish:\n    if: >-\n(.*?)\n    needs:", workflow, re.S).group(1)
        # GitHub implicitly applies success() without a status function. Normal
        # reuse skips must not suppress publication after package has passed.
        if upstream_skipped and "always()" not in condition:
            return False
        expression = " ".join(condition.splitlines()).replace("&&", " and ")
        return eval(expression, {"__builtins__": {}}, dict(always=lambda: True,
            startsWith=lambda value, prefix: value.startswith(prefix),
            github=NS(event_name=event, ref=ref), needs=NS(metadata=NS(result=metadata), package=NS(result=package))))

    def test_tag_publishes_after_reuse_skips(self):
        self.assertTrue(self.allowed())

    def test_manual_and_failed_or_cancelled_packages_never_publish(self):
        self.assertFalse(self.allowed(event="workflow_dispatch"))
        self.assertFalse(self.allowed(ref="refs/heads/master"))
        for result in ("failure", "cancelled", "skipped"):
            self.assertFalse(self.allowed(metadata=result))
            self.assertFalse(self.allowed(package=result))
