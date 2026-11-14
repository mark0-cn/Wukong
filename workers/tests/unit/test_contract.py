"""The same golden values must be accepted/rejected by Rust and Python."""
import json
from pathlib import Path
import unittest
from jsonschema import ValidationError
from worker_contract import CONTRACT, validator, validate_request, validate_result
from worker_rpc import Server


class ContractTests(unittest.TestCase):
    def test_shared_corpus(self):
        path = Path(__file__).resolve().parents[2] / "protocol/fixtures/contract.json"
        for sample in json.loads(path.read_text(encoding="utf-8")):
            with self.subTest(sample=sample):
                try:
                    if sample["kind"] == "request":
                        validate_request(sample["method"], sample["value"])
                    elif sample["kind"] == "result":
                        validate_result(sample["method"], sample["value"])
                    else:
                        validator(CONTRACT[sample["kind"]]).validate(sample["value"])
                    valid = True
                except (ValidationError, ValueError):
                    valid = False
                self.assertEqual(valid, sample["valid"])

    def test_library_rejects_invalid_rpc_without_dispatch(self):
        class Fixture:
            handlers = {"job.start": None}
            def dispatch(self, *_):
                raise AssertionError("invalid request reached executor")
        server = Server(Fixture())
        for request, code in [("{", -32700), ('{"id":1,"method":"job.start"}', -32600),
                              ('{"jsonrpc":"2.0","id":1,"method":"invented"}', -32601),
                              ('{"jsonrpc":"2.0","id":1,"method":"job.start","params":{"timeout_secs":0}}', -32602)]:
            with self.subTest(request=request):
                self.assertEqual(json.loads(server.call(request))["error"]["code"], code)
