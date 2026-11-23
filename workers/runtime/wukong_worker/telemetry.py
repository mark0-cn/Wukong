"""Process-local OpenTelemetry spans with bounded, response-carried OTLP export.

The worker has no telemetry network destination. Completed spans are encoded as
standard OTLP protobuf requests and returned through the RPC envelope only.
"""

import base64
from collections import deque
from dataclasses import dataclass
import json
import re
import threading
from typing import Sequence

from opentelemetry import trace
from opentelemetry.context import Context
from opentelemetry.exporter.otlp.proto.common.trace_encoder import (
    encode_spans as _sdk_encode_spans,
)
from opentelemetry.proto.collector.trace.v1.trace_service_pb2 import (
    ExportTraceServiceRequest,
)
from opentelemetry.sdk.resources import Resource
from opentelemetry.sdk.trace import ReadableSpan, TracerProvider
from opentelemetry.sdk.trace.export import (
    SimpleSpanProcessor,
    SpanExporter,
    SpanExportResult,
)
from opentelemetry.trace import SpanKind
from opentelemetry.trace.propagation.tracecontext import (
    TraceContextTextMapPropagator as TraceContextPropagator,
)

TELEMETRY_FIELD = "_wukong_telemetry"
MAX_TELEMETRY_BYTES = 512 * 1024
MAX_QUEUED_SPANS = 1024
# Base64 expands protobuf by 4/3. The response batch performs the exact JSON
# size check as well, so a full queue may be split across more than one reply.
MAX_QUEUED_BYTES = MAX_TELEMETRY_BYTES * 3 // 4
_TRACEPARENT = re.compile(r"[0-9a-f]{2}-[0-9a-f]{32}-[0-9a-f]{16}-[0-9a-f]{2}")


def _encode_spans(spans: Sequence[ReadableSpan]) -> ExportTraceServiceRequest:
    """Single compatibility boundary for the exporter's semi-private encoder."""
    return _sdk_encode_spans(spans)


@dataclass(frozen=True)
class _QueuedExport:
    request: ExportTraceServiceRequest
    size: int
    span_count: int


class _QueuedSpanExporter(SpanExporter):
    """Retain complete encoded exports; never retain business parameters."""

    def __init__(self, max_spans: int, max_bytes: int) -> None:
        if max_spans < 1 or max_bytes < 1:
            raise ValueError("telemetry queue limits must be positive")
        self.max_spans = max_spans
        self.max_bytes = max_bytes
        self._queue: deque[_QueuedExport] = deque()
        self._span_count = 0
        self._bytes = 0
        self._dropped_spans = 0
        self._lock = threading.Lock()

    @property
    def pending_spans(self) -> int:
        with self._lock:
            return self._span_count

    @property
    def pending_bytes(self) -> int:
        with self._lock:
            return self._bytes

    @property
    def dropped_spans(self) -> int:
        with self._lock:
            return self._dropped_spans

    def export(self, spans: Sequence[ReadableSpan]) -> SpanExportResult:
        if not spans:
            return SpanExportResult.SUCCESS
        try:
            request = _encode_spans(spans)
            size = len(request.SerializeToString())
        except Exception:
            # Telemetry is best effort and must not escape through span.end().
            return SpanExportResult.FAILURE
        count = len(spans)
        with self._lock:
            if self._span_count + count > self.max_spans or self._bytes + size > self.max_bytes:
                self._dropped_spans += count
                # There is no downstream retry destination. Reporting success
                # prevents exporters or callers from interpreting this as RPC failure.
                return SpanExportResult.SUCCESS
            self._queue.append(_QueuedExport(request, size, count))
            self._span_count += count
            self._bytes += size
        return SpanExportResult.SUCCESS

    def pop_batch(self, max_bytes: int = MAX_TELEMETRY_BYTES) -> dict[str, str] | None:
        """Pop the oldest complete exports whose base64 JSON fits ``max_bytes``."""
        with self._lock:
            merged = ExportTraceServiceRequest()
            encoded = None
            used = 0
            for entry in self._queue:
                candidate = ExportTraceServiceRequest()
                candidate.CopyFrom(merged)
                candidate.MergeFrom(entry.request)
                candidate_encoded = base64.b64encode(candidate.SerializeToString()).decode("ascii")
                batch_size = len(
                    json.dumps({"traces": candidate_encoded}, separators=(",", ":")).encode("ascii")
                )
                if batch_size > max_bytes:
                    break
                merged = candidate
                encoded = candidate_encoded
                used += 1
            if encoded is None:
                return None
            for _ in range(used):
                entry = self._queue.popleft()
                self._span_count -= entry.span_count
                self._bytes -= entry.size
            return {"traces": encoded}

    def shutdown(self) -> None:
        # Process exit may discard pending telemetry by design. There is no
        # network flush and no background exporter to join.
        return None

    def force_flush(self, timeout_millis: int = 30000) -> bool:
        return True


class Telemetry:
    """An isolated SDK provider; production uses one process-level instance."""

    def __init__(
        self,
        max_spans: int = MAX_QUEUED_SPANS,
        max_bytes: int = MAX_QUEUED_BYTES,
    ) -> None:
        self.exporter = _QueuedSpanExporter(max_spans, max_bytes)
        self.provider = TracerProvider(
            resource=Resource(attributes={"service.name": "wukong-worker"})
        )
        self.provider.add_span_processor(SimpleSpanProcessor(self.exporter))
        self.tracer = self.provider.get_tracer("wukong_worker.telemetry")
        self._propagator = TraceContextPropagator()

    def extract_context(self, value: object) -> Context:
        """Extract a W3C remote parent, returning an empty context when invalid."""
        empty = Context()
        if not isinstance(value, dict) or not set(value) <= {"traceparent", "tracestate"}:
            return empty
        traceparent = value.get("traceparent")
        tracestate = value.get("tracestate")
        if not isinstance(traceparent, str) or not _TRACEPARENT.fullmatch(traceparent):
            return empty
        if tracestate is not None and (
            not isinstance(tracestate, str)
            or len(tracestate) > 512
            or any(character != " " and not "!" <= character <= "~" for character in tracestate)
        ):
            return empty
        carrier = {"traceparent": traceparent}
        if tracestate is not None:
            carrier["tracestate"] = tracestate
        try:
            extracted = self._propagator.extract(carrier=carrier, context=empty)
            span_context = trace.get_current_span(extracted).get_span_context()
        except (TypeError, ValueError):
            return empty
        if not span_context.is_valid or not span_context.is_remote:
            return empty
        return extracted

    def span(
        self,
        name: str,
        *,
        parent: Context | None = None,
        kind: SpanKind = SpanKind.INTERNAL,
        method: str | None = None,
    ):
        """Start a span without exception events that could disclose RPC data."""
        attributes = {"rpc.system": "jsonrpc"}
        if method is not None:
            attributes["rpc.method"] = method
        return self.tracer.start_as_current_span(
            name,
            context=parent,
            kind=kind,
            attributes=attributes,
            record_exception=False,
            set_status_on_exception=False,
        )

    def pop_batch(self, max_bytes: int = MAX_TELEMETRY_BYTES) -> dict[str, str] | None:
        return self.exporter.pop_batch(max_bytes)


_PROCESS_TELEMETRY = Telemetry()


def get_telemetry() -> Telemetry:
    return _PROCESS_TELEMETRY
