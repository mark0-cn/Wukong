#!/usr/bin/env python3
"""Exercise the loopback OTLP stack and its real query APIs.

This is backend acceptance, not a substitute for a model run, native GUI, or
platform Worker acceptance. It sends only fixed synthetic diagnostic fields.
"""

from __future__ import annotations

import argparse
import json
import secrets
import time
from typing import Any, Callable
from urllib.error import HTTPError, URLError
from urllib.parse import urlencode
from urllib.request import Request, urlopen

COLLECTOR = "http://127.0.0.1:4318"
TEMPO = "http://127.0.0.1:3200"
LOKI = "http://127.0.0.1:3100"
PROMETHEUS = "http://127.0.0.1:9090"
GRAFANA = "http://127.0.0.1:3000"
RESPONSE_CAP = 2 * 1024 * 1024
Json = dict[str, Any] | list[Any]


def request_json(
    url: str,
    *,
    timeout: float,
    payload: dict[str, Any] | None = None,
    parameters: dict[str, str] | None = None,
) -> Json:
    if parameters:
        url = f"{url}?{urlencode(parameters)}"
    body = None if payload is None else json.dumps(payload, separators=(",", ":")).encode()
    request = Request(
        url,
        data=body,
        method="GET" if body is None else "POST",
        headers={"Content-Type": "application/json"} if body is not None else {},
    )
    try:
        with urlopen(request, timeout=timeout) as response:
            if response.status != 200:
                raise RuntimeError(f"request returned HTTP {response.status}")
            content = response.read(RESPONSE_CAP + 1)
    except HTTPError as error:
        raise RuntimeError(f"request returned HTTP {error.code}") from None
    except URLError as error:
        raise RuntimeError(f"backend is unavailable: {error.reason}") from None
    if len(content) > RESPONSE_CAP:
        raise RuntimeError("backend response exceeded the acceptance byte cap")
    value = json.loads(content)
    if not isinstance(value, (dict, list)):
        raise RuntimeError("backend returned a non-collection JSON response")
    return value


def request_object(*args: Any, **kwargs: Any) -> dict[str, Any]:
    value = request_json(*args, **kwargs)
    if not isinstance(value, dict):
        raise RuntimeError("backend returned a non-object JSON response")
    return value


def wait_for(description: str, probe: Callable[[], bool], timeout: float) -> None:
    deadline = time.monotonic() + timeout
    last_error: Exception | None = None
    while time.monotonic() < deadline:
        try:
            if probe():
                return
        except (RuntimeError, ValueError, KeyError, TypeError) as error:
            last_error = error
        time.sleep(1)
    suffix = f": {last_error}" if last_error else ""
    raise RuntimeError(f"timed out waiting for {description}{suffix}")


def attributes(values: dict[str, str]) -> list[dict[str, Any]]:
    return [{"key": key, "value": {"stringValue": value}} for key, value in values.items()]


def export(run_id: str, trace_id: str, start: int, end: int, timeout: float) -> None:
    resource = {"attributes": attributes({"service.name": "wukong"})}
    trace = {
        "resourceSpans": [
            {
                "resource": resource,
                "scopeSpans": [
                    {
                        "scope": {"name": "wukong.acceptance"},
                        "spans": [
                            {
                                "traceId": trace_id,
                                "spanId": secrets.token_hex(8),
                                "name": "wukong.assessment",
                                "kind": 1,
                                "startTimeUnixNano": str(start),
                                "endTimeUnixNano": str(end),
                                "attributes": attributes(
                                    {"run_id": run_id, "outcome": "completed"}
                                ),
                                "status": {"code": "STATUS_CODE_OK"},
                            }
                        ],
                    }
                ],
            }
        ],
    }
    log = {
        "resourceLogs": [
            {
                "resource": resource,
                "scopeLogs": [
                    {
                        "scope": {"name": "wukong.acceptance"},
                        "logRecords": [
                            {
                                "timeUnixNano": str(end),
                                "observedTimeUnixNano": str(end),
                                "severityText": "INFO",
                                "body": {"stringValue": "run_finished"},
                                "attributes": attributes(
                                    {
                                        "run_id": run_id,
                                        "trace_id": trace_id,
                                        "event": "run_finished",
                                        "outcome": "completed",
                                    }
                                ),
                            }
                        ],
                    }
                ],
            }
        ],
    }
    metric = {
        "resourceMetrics": [
            {
                "resource": resource,
                "scopeMetrics": [
                    {
                        "scope": {"name": "wukong.acceptance"},
                        "metrics": [
                            {
                                "name": "wukong.run.started",
                                "sum": {
                                    "aggregationTemporality": "AGGREGATION_TEMPORALITY_DELTA",
                                    "isMonotonic": True,
                                    "dataPoints": [
                                        {
                                            "startTimeUnixNano": str(start),
                                            "timeUnixNano": str(end),
                                            "asInt": "1",
                                            "attributes": attributes({"outcome": "acceptance"}),
                                        }
                                    ],
                                },
                            }
                        ],
                    }
                ],
            }
        ],
    }
    for signal, payload in (("traces", trace), ("logs", log), ("metrics", metric)):
        request_object(f"{COLLECTOR}/v1/{signal}", payload=payload, timeout=timeout)


def verify(run_id: str, trace_id: str, start: int, end: int, timeout: float) -> None:
    seconds = {
        "start": str(start // 1_000_000_000 - 5),
        "end": str(end // 1_000_000_000 + 30),
    }

    def traces() -> bool:
        response = request_object(
            f"{TEMPO}/api/search",
            parameters={**seconds, "q": f'{{ .run_id = "{run_id}" }}', "limit": "20"},
            timeout=timeout,
        )
        return any(
            item.get("traceID", "").lower() == trace_id for item in response.get("traces", [])
        )

    wait_for("Tempo run search", traces, 45)
    detail = request_object(f"{TEMPO}/api/traces/{trace_id}", timeout=timeout)
    if not detail.get("trace", detail).get("batches"):
        raise RuntimeError("Tempo trace detail contained no resource batches")

    def logs() -> bool:
        response = request_object(
            f"{LOKI}/loki/api/v1/query_range",
            parameters={
                "query": f'{{service_name="wukong"}} | run_id="{run_id}" | trace_id="{trace_id}"',
                "start": str(start - 5_000_000_000),
                "end": str(end + 30_000_000_000),
                "direction": "backward",
                "limit": "20",
            },
            timeout=timeout,
        )
        data = response.get("data", {})
        return isinstance(data, dict) and any(
            stream.get("values") for stream in data.get("result", [])
        )

    wait_for("Loki run and trace correlation", logs, 45)

    def metrics() -> bool:
        response = request_object(
            f"{PROMETHEUS}/api/v1/query",
            parameters={"query": 'sum(wukong_run_started_total{outcome="acceptance"})'},
            timeout=timeout,
        )
        data = response.get("data", {})
        return isinstance(data, dict) and any(
            float(item["value"][1]) >= 1 for item in data.get("result", [])
        )

    wait_for("Prometheus controller metric", metrics, 60)
    health = request_object(f"{GRAFANA}/api/health", timeout=timeout)
    dashboard = request_object(
        f"{GRAFANA}/api/dashboards/uid/wukong-observability", timeout=timeout
    )
    sources = request_json(f"{GRAFANA}/api/datasources", timeout=timeout)
    if health.get("database") != "ok":
        raise RuntimeError("Grafana database health is not ok")
    if dashboard.get("dashboard", {}).get("uid") != "wukong-observability":
        raise RuntimeError("Grafana did not provision the Wukong dashboard")
    if not isinstance(sources, list):
        raise RuntimeError("Grafana returned an invalid datasource collection")
    source_ids = {item.get("uid") for item in sources if isinstance(item, dict)}
    if not {"tempo", "loki", "prometheus"}.issubset(source_ids):
        raise RuntimeError("Grafana did not provision all observability datasources")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--timeout", type=float, default=5, help="individual HTTP timeout in seconds"
    )
    arguments = parser.parse_args()
    if not 1 <= arguments.timeout <= 60:
        parser.error("--timeout must be between 1 and 60 seconds")
    run_id = f"observability-acceptance-{int(time.time())}"
    trace_id = secrets.token_hex(16)
    end = time.time_ns()
    start = end - 250_000_000
    export(run_id, trace_id, start, end, arguments.timeout)
    verify(run_id, trace_id, start, end, arguments.timeout)
    print(
        json.dumps(
            {
                "status": "passed",
                "run_id": run_id,
                "trace_id": trace_id,
                "verified": ["otlp", "tempo", "loki", "prometheus", "grafana"],
                "not_covered": ["native GUI", "model analysis", "Worker relay"],
            }
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
