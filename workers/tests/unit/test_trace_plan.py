"""Pure-Python validation and canonicalization tests for dynamic trace plans."""

import copy
import json
from pathlib import Path
import unittest

from wukong_worker.operations.trace_plan import (
    _canonical_json,
    logical_plan,
    plan_signature,
    validate_spec,
)


class TracePlanTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        path = Path(__file__).resolve().parents[2] / "protocol/fixtures/trace_plan.json"
        cls.vector = json.loads(path.read_text(encoding="utf-8"))

    def setUp(self):
        self.spec = copy.deepcopy(self.vector["spec"])

    def assert_invalid(self, spec):
        with self.assertRaises(ValueError):
            validate_spec(spec)

    def test_shared_golden_vector(self):
        normalized = validate_spec(self.spec)
        self.assertEqual(normalized["schema_version"], 1)
        self.assertEqual(normalized["abi"], "sysv_amd64")
        self.assertEqual(normalized["limits"]["max_point_hits"], 256)
        self.assertEqual(_canonical_json(self.spec), self.vector["canonical_json"])
        self.assertEqual(plan_signature(self.spec), self.vector["expected_signature"])
        self.assertEqual(
            json.dumps(
                logical_plan(self.spec),
                ensure_ascii=False,
                sort_keys=True,
                separators=(",", ":"),
            ),
            self.vector["canonical_json"],
        )

    def test_defaults_and_valid_call_site(self):
        module = "a" * 64
        spec = {
            "points": [
                {
                    "kind": "function",
                    "point_id": "f",
                    "location": {
                        "kind": "symbol",
                        "module_sha256": module,
                        "symbol": "f",
                    },
                    "phases": ["return"],
                    "capture": {"return_value": "pointer"},
                },
                {
                    "kind": "call_site",
                    "point_id": "call.f-1",
                    "location": {
                        "kind": "module_offset",
                        "module_sha256": module,
                        "offset": 0,
                    },
                    "capture": {},
                },
            ]
        }
        original = copy.deepcopy(spec)
        normalized = validate_spec(spec)
        self.assertEqual(spec, original)
        self.assertEqual(normalized["limits"]["max_events"], 1024)
        self.assertEqual(normalized["limits"]["max_events_bytes"], 8 * 1024 * 1024)
        self.assertEqual(normalized["limits"]["stop_wait_ms"], 10000)
        self.assertEqual(normalized["points"][0]["capture"]["arguments"], [])
        self.assertEqual(normalized["points"][0]["capture"]["buffers"], [])
        self.assertNotIn("location", logical_plan(spec)["points"][0])
        self.assertNotIn("limits", logical_plan(spec))

    def test_location_and_limits_do_not_change_signature(self):
        relocated = copy.deepcopy(self.spec)
        relocated["points"][0]["location"] = {
            "kind": "module_offset",
            "module_sha256": "b" * 64,
            "offset": (1 << 64) - 1,
        }
        relocated["limits"] = {
            "max_events": 4096,
            "max_point_hits": 1024,
            "max_buffer_bytes": 65536,
            "max_event_bytes": 262144,
            "max_events_bytes": 16 * 1024 * 1024,
            "stop_wait_ms": 30000,
        }
        self.assertEqual(plan_signature(relocated), self.vector["expected_signature"])

    def test_capture_changes_change_signature(self):
        for mutate in (
            lambda spec: spec["points"][0]["capture"]["arguments"][0].update(value_type="i64"),
            lambda spec: spec["points"][0]["capture"]["arguments"][0].update(name="length"),
            lambda spec: spec["points"][0]["capture"].update(return_value="u64"),
            lambda spec: spec["points"][0].update(phases=["entry"]),
        ):
            changed = copy.deepcopy(self.spec)
            mutate(changed)
            if changed["points"][0]["phases"] == ["entry"]:
                changed["points"][0]["capture"].pop("return_value")
            self.assertNotEqual(plan_signature(changed), self.vector["expected_signature"])

    def test_schema_identifier_and_point_validation(self):
        cases = []
        for name, mutate in (
            ("schema", lambda s: s.update(schema_version=2)),
            ("abi", lambda s: s.update(abi="other")),
            ("empty points", lambda s: s.update(points=[])),
            ("bad identifier", lambda s: s["points"][0].update(point_id="0bad")),
            ("space identifier", lambda s: s["points"][0].update(point_id="bad id")),
            ("long identifier", lambda s: s["points"][0].update(point_id="a" * 65)),
            ("empty phases", lambda s: s["points"][0].update(phases=[])),
            ("duplicate phases", lambda s: s["points"][0].update(phases=["entry", "entry"])),
            ("unknown phase", lambda s: s["points"][0].update(phases=["exit"])),
            ("non-string phase", lambda s: s["points"][0].update(phases=[[]])),
            ("return without phase", lambda s: s["points"][0].update(phases=["entry"])),
        ):
            spec = copy.deepcopy(self.spec)
            mutate(spec)
            cases.append((name, spec))
        duplicate = copy.deepcopy(self.spec)
        duplicate["points"].append(copy.deepcopy(duplicate["points"][0]))
        cases.append(("duplicate point id", duplicate))
        too_many = copy.deepcopy(self.spec)
        point = too_many["points"][0]
        too_many["points"] = []
        for index in range(65):
            copy_point = copy.deepcopy(point)
            copy_point["point_id"] = f"p{index}"
            too_many["points"].append(copy_point)
        cases.append(("too many points", too_many))
        call_site_return = copy.deepcopy(self.spec)
        call_site_return["points"][0].pop("phases")
        call_site_return["points"][0]["kind"] = "call_site"
        cases.append(("call-site return", call_site_return))
        for name, spec in cases:
            with self.subTest(name=name):
                self.assert_invalid(spec)

    def test_capture_validation(self):
        cases = []
        for name, mutate in (
            ("argument index", lambda s: s["points"][0]["capture"]["arguments"][0].update(index=6)),
            (
                "boolean index",
                lambda s: s["points"][0]["capture"]["arguments"][0].update(index=True),
            ),
            (
                "empty capture name",
                lambda s: s["points"][0]["capture"]["arguments"][0].update(name=""),
            ),
            (
                "long capture name",
                lambda s: s["points"][0]["capture"]["arguments"][0].update(name="x" * 65),
            ),
            (
                "bad integer type",
                lambda s: s["points"][0]["capture"]["arguments"][0].update(value_type="float"),
            ),
            (
                "buffer address index",
                lambda s: s["points"][0]["capture"]["buffers"][0].update(address_argument=6),
            ),
            (
                "buffer length index",
                lambda s: s["points"][0]["capture"]["buffers"][0].update(
                    length={"kind": "argument", "index": 6}
                ),
            ),
            (
                "constant overflow",
                lambda s: s["points"][0]["capture"]["buffers"][0].update(
                    length={"kind": "constant", "bytes": 1024 * 1024 + 1}
                ),
            ),
            ("bad return type", lambda s: s["points"][0]["capture"].update(return_value="void")),
        ):
            spec = copy.deepcopy(self.spec)
            mutate(spec)
            cases.append((name, spec))
        duplicate_indices = copy.deepcopy(self.spec)
        duplicate_indices["points"][0]["capture"]["arguments"][1]["index"] = 1
        cases.append(("duplicate argument index", duplicate_indices))
        duplicate_names = copy.deepcopy(self.spec)
        duplicate_names["points"][0]["capture"]["buffers"][0]["name"] = "size"
        cases.append(("capture name shared namespace", duplicate_names))
        for name, spec in cases:
            with self.subTest(name=name):
                self.assert_invalid(spec)

    def test_location_hash_symbol_and_overflow_validation(self):
        for symbol in ("", "main+4", "main()", "x=1", "x;id", "x\nid", "x|id", "x&y"):
            spec = copy.deepcopy(self.spec)
            spec["points"][0]["location"]["symbol"] = symbol
            with self.subTest(symbol=symbol):
                self.assert_invalid(spec)
        for module_sha256 in ("A" * 64, "a" * 63, "g" * 64):
            spec = copy.deepcopy(self.spec)
            spec["points"][0]["location"]["module_sha256"] = module_sha256
            with self.subTest(module_sha256=module_sha256):
                self.assert_invalid(spec)
        spec = copy.deepcopy(self.spec)
        spec["points"][0]["location"] = {
            "kind": "module_offset",
            "module_sha256": "a" * 64,
            "offset": 1 << 64,
        }
        self.assert_invalid(spec)

    def test_limit_validation(self):
        ranges = {
            "max_events": (0, 4097),
            "max_point_hits": (0, 1025),
            "max_buffer_bytes": (0, 65537),
            "max_event_bytes": (1023, 262145),
            "max_events_bytes": (4095, 16 * 1024 * 1024 + 1),
            "stop_wait_ms": (99, 30001),
        }
        for field, values in ranges.items():
            for value in values:
                spec = copy.deepcopy(self.spec)
                spec["limits"] = {field: value}
                with self.subTest(field=field, value=value):
                    self.assert_invalid(spec)
        spec = copy.deepcopy(self.spec)
        spec["limits"] = {"max_event_bytes": 65537, "max_events_bytes": 65536}
        self.assert_invalid(spec)

    def test_unknown_fields_and_invalid_containers(self):
        mutations = (
            lambda s: s.update(unknown=True),
            lambda s: s["points"][0].update(unknown=True),
            lambda s: s["points"][0]["location"].update(unknown=True),
            lambda s: s["points"][0]["capture"].update(unknown=True),
            lambda s: s["points"][0]["capture"]["arguments"][0].update(unknown=True),
            lambda s: s["points"][0]["capture"]["buffers"][0].update(unknown=True),
            lambda s: s["points"][0]["capture"]["buffers"][0]["length"].update(unknown=True),
            lambda s: s.update(limits={"unknown": 1}),
            lambda s: s.update(limits=None),
            lambda s: s.update(points={}),
        )
        for mutate in mutations:
            spec = copy.deepcopy(self.spec)
            mutate(spec)
            with self.subTest(spec=spec):
                self.assert_invalid(spec)


if __name__ == "__main__":
    unittest.main()
