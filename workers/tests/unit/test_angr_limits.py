"""Real Explorer checks: initially matching states and bounded loops."""
import importlib.util
import unittest

from angr_solve import bounded_explore


@unittest.skipUnless(importlib.util.find_spec("angr"), "angr is not installed")
class AngrLimitTests(unittest.TestCase):
    def simulation(self):
        import angr
        project = angr.load_shellcode(b"\xeb\xfe", arch="amd64", load_address=0x400000)
        return project.factory.simulation_manager(project.factory.entry_state(addr=0x400000))

    def test_unreachable_target_stops_at_step_limit(self):
        manager, steps = bounded_explore(self.simulation(), [0x400010], [], 3)
        self.assertEqual(steps, 3)
        self.assertFalse(manager.found)
        self.assertEqual(len(manager.active), 1)

    def test_initial_target_is_found_without_executing_loop(self):
        manager, steps = bounded_explore(self.simulation(), [0x400000], [], 3)
        self.assertEqual(len(manager.found), 1)
        self.assertEqual(steps, 1)
        self.assertEqual(manager.found[0].history.block_count, 0)

    def test_avoided_state_is_not_explored(self):
        manager, steps = bounded_explore(self.simulation(), [0x400010], [0x400000], 3)
        self.assertEqual(steps, 1)
        self.assertFalse(manager.found)
        self.assertFalse(manager.active)
        self.assertEqual(len(manager.avoid), 1)
