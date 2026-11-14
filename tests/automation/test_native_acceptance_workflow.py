"""Exercise the actual workflow's independent native-acceptance conditions."""
from pathlib import Path
import re
from types import SimpleNamespace as NS
import unittest


class NativeAcceptanceConditions(unittest.TestCase):
    @staticmethod
    def block(name):
        workflow = Path('.github/workflows/ci.yml').read_text(encoding='utf-8')
        return next(block for block in workflow.split('      - name: ')
                    if block.startswith(name + '\n'))

    def enabled(self, name, *, previous_failure=False, cancelled=False,
                rust='true', native='success', fixture='skipped', selected=True,
                labeled=False):
        condition = re.search(r'^        if: (.+)$', self.block(name), re.M).group(1)
        # GitHub adds success() when no explicit status-check function appears.
        if previous_failure and not re.search(r'\b(?:always|cancelled|failure|success)\(', condition):
            return False
        expression = condition.removeprefix('${{').removesuffix('}}').strip()
        expression = expression.replace('!cancelled()', 'not cancelled()')
        expression = expression.replace('&&', ' and ').replace('||', ' or ')
        expression = expression.replace('== true', '== True')
        expression = expression.replace('github.event.pull_request.labels.*.name', 'labels')
        return eval(expression, {'__builtins__': {}}, dict(
            always=lambda: True, cancelled=lambda: cancelled,
            contains=lambda items, value: value in items,
            labels=['windows-worker-acceptance'] if labeled else [],
            inputs=NS(windows_worker_acceptance=selected),
            needs=NS(changes=NS(outputs=NS(rust=rust))),
            steps=NS(worker_native=NS(outcome=native), worker_acceptance=NS(outcome=fixture))))

    def test_unrelated_failure_does_not_skip_selected_native_evidence(self):
        for previous_failure in [False, True]:
            self.assertTrue(self.enabled('Standalone Windows worker bootstrap',
                                         previous_failure=previous_failure))
            for selected, labeled in [(True, False), (False, True)]:
                self.assertTrue(self.enabled('Dedicated hosted Windows worker installation acceptance',
                    previous_failure=previous_failure, selected=selected, labeled=labeled))

    def test_cancellation_native_failure_and_opt_out_prevent_installation(self):
        self.assertFalse(self.enabled('Standalone Windows worker bootstrap', cancelled=True))
        self.assertFalse(self.enabled('Standalone Windows worker bootstrap', rust='false'))
        for settings in [dict(cancelled=True), dict(selected=False),
                         *[dict(native=status) for status in ['failure', 'cancelled', 'skipped', '']]]:
            self.assertFalse(self.enabled('Dedicated hosted Windows worker installation acceptance',
                                          previous_failure=True, **settings))

    def test_report_requires_a_finished_fixture_and_failures_remain_failures(self):
        for outcome in ['success', 'failure', 'skipped', 'cancelled', '']:
            self.assertEqual(self.enabled('Preserve public native installation report',
                previous_failure=True, fixture=outcome), outcome in {'success', 'failure'})
        for name in ['Rust tests', 'Standalone Windows worker bootstrap',
                     'Dedicated hosted Windows worker installation acceptance']:
            self.assertNotIn('continue-on-error:', self.block(name))
