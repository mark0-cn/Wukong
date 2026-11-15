"""Shared v3 contracts generated from workers/protocol Rust types."""
import json
from pathlib import Path
from jsonschema import Draft202012Validator

CONTRACT = json.loads((Path(__file__).resolve().parent.parent / "protocol/schema/contract.json").read_text(encoding="utf-8"))
PROTOCOL = CONTRACT["protocol"]
METHODS = CONTRACT["methods"]
CONTEXT_KEYS = frozenset(CONTRACT["context"]["properties"])


def validator(schema):
    Draft202012Validator.check_schema(schema)
    return Draft202012Validator(schema)


CONTEXT = validator(CONTRACT["context"])
INPUTS = {name: validator(spec["params"]) for name, spec in METHODS.items()}
OUTPUTS = {name: validator(spec["result"]) for name, spec in METHODS.items()}


def validate_request(method, params):
    if not isinstance(params, dict):
        raise ValueError("worker parameters must be an object")
    CONTEXT.validate({key: value for key, value in params.items() if key in CONTEXT_KEYS})
    INPUTS[method].validate({key: value for key, value in params.items() if key not in CONTEXT_KEYS})


def validate_result(method, result):
    OUTPUTS[method].validate(result)
