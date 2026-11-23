"""JSON-RPC dispatch, telemetry envelope boundaries and standard errors."""

from dataclasses import dataclass
import json

from jsonrpcserver import dispatch, Success, Error
from jsonschema import ValidationError
from opentelemetry.context import Context
from opentelemetry.trace import SpanKind
from wukong_worker.core.contract import MAX_RPC as MAX_RPC_BYTES
from wukong_worker.core.errors import error_kind
from wukong_worker.core.contract import validate_request, validate_result
from wukong_worker.telemetry import (
    MAX_TELEMETRY_BYTES,
    TELEMETRY_FIELD,
    Telemetry,
    get_telemetry,
)


@dataclass(frozen=True)
class RequestEnvelope:
    """A cleaned raw request and its explicit, non-global remote parent."""

    message: str
    context: Context
    method: str | None


def _json_object(message: str) -> dict | None:
    try:
        value = json.loads(message)
    except (json.JSONDecodeError, TypeError):
        return None
    return value if isinstance(value, dict) else None


def extract_request_context(request: str, telemetry: Telemetry) -> RequestEnvelope:
    """Remove only the top-level extension before jsonrpcserver sees a request."""
    envelope = _json_object(request)
    if envelope is None:
        return RequestEnvelope(request, Context(), None)
    method = envelope.get("method")
    if TELEMETRY_FIELD not in envelope:
        return RequestEnvelope(request, Context(), method if isinstance(method, str) else None)
    raw_context = envelope.pop(TELEMETRY_FIELD)
    cleaned = json.dumps(envelope, ensure_ascii=False, separators=(",", ":"))
    return RequestEnvelope(
        cleaned,
        telemetry.extract_context(raw_context),
        method if isinstance(method, str) else None,
    )


def attach_response_telemetry(
    response: str,
    batch: dict[str, str] | None,
    max_rpc_bytes: int = MAX_RPC_BYTES,
) -> str:
    """Attach a complete batch or preserve the original business response."""
    if not batch:
        return response
    envelope = _json_object(response)
    if envelope is None:
        return response
    batch_bytes = len(json.dumps(batch, separators=(",", ":")).encode("ascii"))
    if batch_bytes > MAX_TELEMETRY_BYTES:
        return response
    envelope[TELEMETRY_FIELD] = batch
    attached = json.dumps(envelope, ensure_ascii=False, separators=(",", ":"))
    if len(attached.encode("utf-8")) > max_rpc_bytes:
        return response
    return attached


class Server:
    def __init__(self, runtime, telemetry: Telemetry | None = None):
        self.runtime = runtime
        self.telemetry = (
            telemetry
            if telemetry is not None
            else getattr(runtime, "telemetry", None) or get_telemetry()
        )
        self.methods = {name: self.handler(name) for name in runtime.handlers}

    def handler(self, name):
        def invoke(**params):
            try:
                validate_request(name, params)
            except (ValidationError, ValueError) as error:
                return Error(-32602, str(error)[:2048], {"kind": "invalid_request"})
            try:
                result = self.runtime.dispatch(name, params)
                validate_result(name, result)
                return Success(result)
            except Exception as error:
                # This is the protocol boundary: unexpected implementation errors
                # still become bounded RPC errors, never traceback text on stdout.
                return Error(-32000, str(error)[:4096], {"kind": error_kind(error)})

        return invoke

    def call(self, request: str) -> str:
        envelope = extract_request_context(request, self.telemetry)
        method = envelope.method if envelope.method in self.methods else None
        with self.telemetry.span(
            "rpc.server",
            parent=envelope.context,
            kind=SpanKind.SERVER,
            method=method,
        ):
            response = dispatch(envelope.message, methods=self.methods, max_batch_size=1)
        # Reserve the actual JSON field overhead and never reduce the business
        # response. Entries that do not fit remain queued for a later response.
        field_overhead = len((',"' + TELEMETRY_FIELD + '":').encode("ascii"))
        available = min(
            MAX_TELEMETRY_BYTES,
            MAX_RPC_BYTES - len(response.encode("utf-8")) - field_overhead,
        )
        batch = self.telemetry.pop_batch(available) if available > 0 else None
        try:
            return attach_response_telemetry(response, batch, MAX_RPC_BYTES)
        except Exception:
            # Encoding telemetry must never replace a valid business response.
            return response
