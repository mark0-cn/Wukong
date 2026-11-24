"""Read the controller/Worker compatibility version from the generated contract."""

import json

from scripts._lib.repository import ROOT

RUNTIME_PROTOCOL = json.loads(
    (ROOT / "workers/protocol/schema/contract.json").read_text(encoding="utf-8")
)["protocol"]
if type(RUNTIME_PROTOCOL) is not int or RUNTIME_PROTOCOL < 1:
    raise ValueError("Worker protocol contract must declare a positive integer")
