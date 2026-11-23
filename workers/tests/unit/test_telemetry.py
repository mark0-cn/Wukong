"""Worker-local W3C propagation and bounded OTLP response envelopes."""

import base64
from copy import deepcopy
import json
from pathlib import Path
import tempfile
import threading
import unittest
from unittest.mock import patch

from opentelemetry.proto.collector.trace.v1.trace_service_pb2 import (
    ExportTraceServiceRequest,
)

from wukong_worker.host.runtime import Runtime
from wukong_worker.rpc.server import Server, attach_response_telemetry
from wukong_worker.telemetry import (
    MAX_QUEUED_BYTES,
    MAX_TELEMETRY_BYTES,
    TELEMETRY_FIELD,
    Telemetry,
)

TRACE_ID = "4bf92f3577b34da6a3ce929d0e0e4736"
PARENT_ID = "00f067aa0ba902b7"
OTHER_TRACE_ID = "7bf92f3577b34da6a3ce929d0e0e4736"
OTHER_PARENT_ID = "10f067aa0ba902b7"
TRACEPARENT = f"00-{TRACE_ID}-{PARENT_ID}-01"


def request(method="job.start", params=None, context=None, identifier=1):
    envelope = {
        "jsonrpc": "2.0",
        "id": identifier,
        "method": method,
        "params": {} if params is None else params,
    }
    if context is not None:
        envelope[TELEMETRY_FIELD] = context
    return json.dumps(envelope)


def decode_batch(batch):
    decoded = ExportTraceServiceRequest()
    decoded.ParseFromString(base64.b64decode(batch["traces"], validate=True))
    return decoded


def spans(batch):
    decoded = decode_batch(batch)
    return [
        span
        for resource in decoded.resource_spans
        for scope in resource.scope_spans
        for span in scope.spans
    ]


def split_response(response):
    envelope = json.loads(response)
    batch = envelope.pop(TELEMETRY_FIELD)
    return envelope, batch


class FixtureRuntime:
    handlers = {"job.start": None}

    def __init__(self):
        self.calls = []

    def dispatch(self, method, params):
        self.calls.append((method, deepcopy(params)))
        return {"job_id": "fixture-job", "state": "queued"}


class TelemetryEnvelopeTests(unittest.TestCase):
    def test_valid_remote_parent_and_params_survive_response_strip(self):
        telemetry = Telemetry()
        runtime = FixtureRuntime()
        server = Server(runtime, telemetry)
        params = {
            "argv": ["fixture-target", "--safe"],
            "env": {"FIXTURE_SECRET": "not-telemetry"},
            "request_id": "request-stable",
            "call_id": "call-stable",
        }
        raw = server.call(
            request(
                "job.start",
                params,
                {"traceparent": TRACEPARENT, "tracestate": "vendor=abc"},
            )
        )
        business, batch = split_response(raw)
        self.assertEqual(business["id"], 1)
        self.assertEqual(business["result"]["job_id"], "fixture-job")
        self.assertEqual(runtime.calls, [("job.start", params)])
        exported = spans(batch)
        self.assertEqual(len(exported), 1)
        server_span = exported[0]
        self.assertEqual(server_span.name, "rpc.server")
        self.assertEqual(server_span.trace_id.hex(), TRACE_ID)
        self.assertEqual(len(server_span.span_id), 8)
        self.assertNotEqual(server_span.span_id, b"\0" * 8)
        self.assertEqual(server_span.parent_span_id.hex(), PARENT_ID)
        self.assertEqual(server_span.trace_state, "vendor=abc")
        self.assertEqual(server_span.kind, 2)  # OTLP SPAN_KIND_SERVER
        attributes = {item.key: item.value.string_value for item in server_span.attributes}
        self.assertEqual(attributes["rpc.method"], "job.start")
        protobuf = base64.b64decode(batch["traces"])
        self.assertNotIn(b"fixture-target", protobuf)
        self.assertNotIn(b"not-telemetry", protobuf)

        # A request without context must not inherit the preceding request's parent.
        following, following_batch = split_response(server.call(request(identifier=2)))
        self.assertNotIn("error", following)
        following_span = spans(following_batch)[0]
        self.assertNotEqual(following_span.trace_id.hex(), TRACE_ID)
        self.assertEqual(following_span.parent_span_id, b"")

    def test_missing_and_invalid_context_never_break_rpc(self):
        invalid = [
            None,
            "invalid",
            {},
            {"traceparent": "invalid"},
            {"traceparent": TRACEPARENT, "unknown": "field"},
            {
                "traceparent": TRACEPARENT.upper(),
            },
            {"traceparent": "00-00000000000000000000000000000000-00f067aa0ba902b7-01"},
            {"traceparent": " " + TRACEPARENT},
            {"traceparent": TRACEPARENT, "tracestate": "bad\nvalue"},
            {"traceparent": TRACEPARENT, "tracestate": "vendor=秘密"},
            {"traceparent": TRACEPARENT, "tracestate": 7},
        ]
        for context in invalid:
            with self.subTest(context=context):
                telemetry = Telemetry()
                runtime = FixtureRuntime()
                envelope = json.loads(request())
                envelope[TELEMETRY_FIELD] = context
                response = json.loads(Server(runtime, telemetry).call(json.dumps(envelope)))
                self.assertNotIn("error", response)
                batch = response.pop(TELEMETRY_FIELD)
                server_span = spans(batch)[0]
                self.assertEqual(server_span.parent_span_id, b"")
                self.assertEqual(runtime.calls[0][1], {})

    def test_runtime_spans_and_idempotent_result_do_not_depend_on_context(self):
        with tempfile.TemporaryDirectory() as directory:
            telemetry = Telemetry()
            runtime = Runtime(Path(directory), trusted=True, telemetry=telemetry)
            calls = []

            def start(params):
                calls.append(deepcopy(params))
                return {"job_id": "idempotent-fixture", "state": "queued"}

            runtime.handlers["job.start"] = start
            server = Server(runtime, telemetry)
            params = {
                "argv": ["secret-target", "--private"],
                "env": {"FIXTURE_TOKEN": "secret-value"},
                "request_id": "telemetry-idempotency",
            }
            first_business, first_batch = split_response(
                server.call(
                    request(
                        "job.start",
                        params,
                        {"traceparent": TRACEPARENT},
                        identifier=11,
                    )
                )
            )
            second_parent = f"00-{OTHER_TRACE_ID}-{OTHER_PARENT_ID}-01"
            second_business, second_batch = split_response(
                server.call(
                    request(
                        "job.start",
                        params,
                        {"traceparent": second_parent},
                        identifier=12,
                    )
                )
            )
            self.assertEqual(first_business["result"], second_business["result"])
            self.assertEqual(calls, [params])

            first_spans = {span.name: span for span in spans(first_batch)}
            self.assertEqual(set(first_spans), {"rpc.server", "worker.dispatch", "job.start"})
            self.assertEqual(first_spans["rpc.server"].trace_id.hex(), TRACE_ID)
            for span in first_spans.values():
                self.assertEqual(span.trace_id.hex(), TRACE_ID)
            self.assertEqual(
                first_spans["worker.dispatch"].parent_span_id,
                first_spans["rpc.server"].span_id,
            )
            self.assertEqual(
                first_spans["job.start"].parent_span_id,
                first_spans["worker.dispatch"].span_id,
            )
            for span in spans(second_batch):
                self.assertEqual(span.trace_id.hex(), OTHER_TRACE_ID)
            protobuf = base64.b64decode(first_batch["traces"])
            self.assertNotIn(b"secret-target", protobuf)
            self.assertNotIn(b"secret-value", protobuf)


class TelemetryQueueTests(unittest.TestCase):
    def test_count_and_byte_overflow_drop_only_bounded_telemetry(self):
        telemetry = Telemetry(max_spans=2, max_bytes=MAX_QUEUED_BYTES)
        for name in ["one", "two", "dropped"]:
            with telemetry.span(name):
                pass
        self.assertEqual(telemetry.exporter.pending_spans, 2)
        self.assertEqual(telemetry.exporter.dropped_spans, 1)
        batch = telemetry.pop_batch()
        self.assertEqual([span.name for span in spans(batch)], ["one", "two"])
        self.assertEqual(telemetry.exporter.pending_spans, 0)
        self.assertEqual(telemetry.exporter.pending_bytes, 0)

        byte_limited = Telemetry(max_spans=10, max_bytes=1)
        with byte_limited.span("too-large"):
            pass
        self.assertEqual(byte_limited.exporter.pending_spans, 0)
        self.assertEqual(byte_limited.exporter.dropped_spans, 1)
        self.assertIsNone(byte_limited.pop_batch())

    def test_batch_size_limit_retains_complete_standard_protobuf_exports(self):
        telemetry = Telemetry()
        for name in ["first", "second"]:
            with telemetry.span(name):
                pass
        self.assertIsNone(telemetry.pop_batch(32))
        self.assertEqual(telemetry.exporter.pending_spans, 2)
        batch = telemetry.pop_batch(MAX_TELEMETRY_BYTES)
        encoded_batch = json.dumps(batch, separators=(",", ":")).encode("ascii")
        self.assertLessEqual(len(encoded_batch), MAX_TELEMETRY_BYTES)
        self.assertEqual([span.name for span in spans(batch)], ["first", "second"])

    def test_only_allowlisted_attributes_and_no_freeform_events_are_exported(self):
        telemetry = Telemetry()
        with telemetry.span("rpc.server", method="job.start"):
            pass
        decoded = decode_batch(telemetry.pop_batch())
        allowed_resource = {
            "service.name",
            "service.version",
            "telemetry.sdk.name",
            "telemetry.sdk.language",
            "telemetry.sdk.version",
            "os.type",
            "process.runtime.name",
            "process.runtime.version",
        }
        for resource in decoded.resource_spans:
            self.assertTrue({item.key for item in resource.resource.attributes} <= allowed_resource)
            for scope in resource.scope_spans:
                for span in scope.spans:
                    self.assertEqual(
                        {item.key for item in span.attributes},
                        {"rpc.system", "rpc.method"},
                    )
                    self.assertEqual(len(span.events), 0)
                    self.assertEqual(len(span.links), 0)
                    self.assertEqual(span.status.message, "")

    def test_rpc_size_limit_preserves_business_response_and_pending_spans(self):
        telemetry = Telemetry()
        server = Server(FixtureRuntime(), telemetry)
        with patch("wukong_worker.rpc.server.MAX_RPC_BYTES", 1):
            response = server.call(request(context={"traceparent": TRACEPARENT}))
        business = json.loads(response)
        self.assertNotIn(TELEMETRY_FIELD, business)
        self.assertNotIn("error", business)
        self.assertEqual(telemetry.exporter.pending_spans, 1)

        _, batch = split_response(server.call(request(identifier=2)))
        self.assertEqual(len(spans(batch)), 2)
        self.assertEqual(telemetry.exporter.pending_spans, 0)

    def test_response_attachment_enforces_both_size_limits(self):
        telemetry = Telemetry()
        with telemetry.span("fixture"):
            pass
        batch = telemetry.pop_batch()
        response = '{"jsonrpc":"2.0","id":1,"result":{"ok":true}}'
        attached = attach_response_telemetry(response, batch, 1024 * 1024)
        business, attached_batch = split_response(attached)
        self.assertEqual(business, json.loads(response))
        self.assertEqual(attached_batch, batch)
        self.assertEqual(attach_response_telemetry(response, batch, len(response) + 8), response)
        oversized = {"traces": "A" * (MAX_TELEMETRY_BYTES * 2)}
        self.assertEqual(
            attach_response_telemetry(response, oversized, 10 * 1024 * 1024),
            response,
        )

    def test_background_thread_does_not_inherit_ambient_request_context(self):
        telemetry = Telemetry()
        with telemetry.span("request"):
            # Use an explicit context manager in the thread so it always ends.
            def background():
                with telemetry.span("background"):
                    pass

            worker = threading.Thread(target=background)
            worker.start()
            worker.join()
        exported = {span.name: span for span in spans(telemetry.pop_batch())}
        self.assertNotEqual(exported["background"].trace_id, exported["request"].trace_id)
        self.assertEqual(exported["background"].parent_span_id, b"")


if __name__ == "__main__":
    unittest.main()
