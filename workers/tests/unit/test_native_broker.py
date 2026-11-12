"""The native health channel is an identity check, never execution admission."""
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

import worker_native_broker


class NativeBrokerTests(unittest.TestCase):
    def test_fixed_verified_client_requires_exact_identity_and_never_enables_execution(self):
        identity = {"protocol": 2, "installation_id": "a"*32, "boot_id": "windows:"+"b"*32+":13", "package_sha256": "c"*64}
        with tempfile.TemporaryDirectory() as temporary:
            client = Path(temporary)/"native-client.exe"
            client.write_bytes(b"inert-native-client-fixture")
            environment = {"WUKONG_NATIVE_BROKER_"+key: value for key,value in {
                "CLIENT": str(client), "CLIENT_SHA256": hashlib.sha256(client.read_bytes()).hexdigest(),
                "INSTALLATION": "a"*32, "PARENT_PID": "123"}.items()}
            response = {"schema": 1, "identity": identity, "production_execution": False, "scope": "native_broker_channel_only"}
            call = Mock()
            process = SimpleNamespace(run=call, TimeoutExpired=subprocess.TimeoutExpired)
            with patch.object(worker_native_broker, "os", SimpleNamespace(name="nt", environ=environment)), \
                    patch.object(worker_native_broker, "subprocess", process):
                call.return_value = subprocess.CompletedProcess([], 0, json.dumps(response).encode(), b"")
                result = worker_native_broker.doctor(identity)
                self.assertTrue(result["available"])
                self.assertFalse(result["production_execution"])
                self.assertEqual(call.call_args.args[0], [str(client), "broker-health", "a"*32, "123"])
                for invalid in [{**response, "production_execution": True},
                                {**response, "identity": {**identity, "package_sha256": "d"*64}},
                                {**response, "arbitrary": "field"}, None, [], [{"invalid": True}]]:
                    call.return_value = subprocess.CompletedProcess([], 0, json.dumps(invalid).encode(), b"")
                    self.assertFalse(worker_native_broker.doctor(identity)["available"])
                call.return_value = subprocess.CompletedProcess([], 0, b"x"*4097, b"")
                self.assertFalse(worker_native_broker.doctor(identity)["available"])
                environment["WUKONG_NATIVE_BROKER_CLIENT_SHA256"] = "0"*64
                call.reset_mock()
                self.assertFalse(worker_native_broker.doctor(identity)["available"])
                call.assert_not_called()
