<a id="observability"></a>

# Observability

English · [简体中文](observability.zh-CN.md)

[User guide](usage.md) · [Documentation index](../README.md)

Wukong can export OpenTelemetry signals and query them from the desktop. This guide covers configuration, local activity, Worker trace propagation, the optional development stack, and operational limits.

<a id="scope-and-source-of-truth"></a>

## Scope and source of truth

The durable business Store, Events, Jobs, receipts, original evidence, and verification records remain authoritative. Telemetry is best-effort operational data. A missing span, log, metric, or backend never proves that work did not happen, and a successful telemetry export never proves business success.

Telemetry does not replace completion gates, evidence readiness, resource-cleanup confirmation, independent verification, recovery, or reconciliation. Use the existing workspace, job/resource records, and original evidence for those decisions. The desktop's **Local activity** panel is a bounded projection of the durable Store, not an OpenTelemetry trace.

<a id="signals-and-privacy"></a>

## Signals and privacy

The controller supports traces, metrics, optional OpenTelemetry logs, and local JSONL logs:

- **Traces** include Wukong operations and reuse native Rig spans from `rig`, `rig_core`, `rig_agent`, and `rig_ecs`. Run, Agent, generation, turn, call, Worker request, and job IDs can correlate spans and curated logs.
- **Metrics** cover runs, Agent generations, Worker RPC, model operations/token usage, evidence synchronization, and dropped telemetry. They use low-cardinality operational labels. Run, Agent, call, and other business record IDs are deliberately not metric labels, so metrics cannot identify one run or Agent.
- **OpenTelemetry logs are opt-in** (`logs = false` by default). Both OTLP logs and local JSONL accept only events explicitly emitted to the curated `wukong_telemetry` target. Arbitrary application diagnostics, `RUST_LOG` output, and arbitrary event fields are not forwarded to those sinks.
- **Local JSONL is independently enabled by default**. It writes daily `controller.jsonl*` files under `state_dir/logs`, using a bounded, lossy, nonblocking writer. It is an operational log, not an evidence archive.

Sensitive-content capture is not enabled by default. These settings do not provide a switch to export raw prompts, completions, private reasoning, tool arguments/results, target bytes, credentials, or full backend error bodies. Standard errors remain available through their existing bounded diagnostic paths; the curated telemetry sinks are not a general-purpose verbose-log export.

Query adapters also filter returned attributes and labels, bound response sizes, redact an echoed configured bearer token, and return fixed safe errors rather than backend response bodies. Treat run IDs and correlation IDs as potentially sensitive metadata and apply appropriate access control and retention to external backends.

<a id="configuration"></a>

## Configuration

Configuration lives under `[observability]`. Export and query are separate: configuring a query backend does not enable export, and enabling a signal has no OTLP destination until an endpoint is configured.

<a id="complete-example"></a>

### Complete example

This example matches the loopback development stack below. Omit `credential_env` for that credential-free stack. `logs = true` is required if Loki/OpenTelemetry log export is wanted.

```toml
[observability]
service_name = "wukong"
otlp_endpoint = "http://127.0.0.1:4318"
traces = true
metrics = true
logs = true
local_jsonl = true
log_retention_days = 7
shutdown_timeout_secs = 3

[observability.query]
grafana_url = "http://127.0.0.1:3000"
timeout_secs = 5
max_range_secs = 86400

[observability.query.tempo]
url = "http://127.0.0.1:3200"
# credential_env = "WUKONG_TEMPO_TOKEN"

[observability.query.loki]
url = "http://127.0.0.1:3100"
# credential_env = "WUKONG_LOKI_TOKEN"

[observability.query.prometheus]
url = "http://127.0.0.1:9090"
# credential_env = "WUKONG_PROMETHEUS_TOKEN"
```

| Field | Default | Meaning and accepted range |
| --- | --- | --- |
| `service_name` | `"wukong"` | OpenTelemetry service name; nonempty, at most 128 bytes. |
| `otlp_endpoint` | unset | Base OTLP/HTTP endpoint. The controller appends the signal path. |
| `traces` | `true` | Enable trace export when an endpoint is configured. |
| `metrics` | `true` | Enable metric export when an endpoint is configured. |
| `logs` | `false` | Enable curated OTLP log export when an endpoint is configured. |
| `local_jsonl` | `true` | Enable curated local JSONL independently of OTLP export. |
| `log_retention_days` | `7` | Local JSONL retention, 1–90 days. |
| `shutdown_timeout_secs` | `3` | One total telemetry drain deadline, 1–30 seconds. |
| `query.tempo`, `query.loki`, `query.prometheus` | unset | Optional read-only backend records with `url` and optional `credential_env`. |
| `query.grafana_url` | unset | Base Grafana URL used to construct Explore deep links. |
| `query.timeout_secs` | `5` | Backend query timeout, 1–60 seconds. |
| `query.max_range_secs` | `86400` | Maximum query range, 60 seconds through 31 days. |

In desktop settings, `credential_env` must contain only the **name of an environment variable**, never a bearer token. The controller reads that variable and sends its value as an HTTP Bearer credential. A missing or invalid variable degrades only the affected query backend. Secrets must remain in environment variables, not TOML, endpoint URLs, Grafana links, screenshots, or committed Compose files.

Every export and query endpoint must be an absolute HTTPS URL or loopback HTTP URL (`localhost`, `127.0.0.1`, or `::1`). Userinfo, embedded credentials, query parameters, and fragments are rejected. Use HTTPS for every non-loopback destination.

<a id="export-environment-variables-and-precedence"></a>

### Export environment variables and precedence

| Variable | Behavior |
| --- | --- |
| `OTEL_SDK_DISABLED` | The case-insensitive value `true` disables configured OTLP export and the Worker relay. Local JSONL and saved query configuration remain independent. |
| `OTEL_SERVICE_NAME` | Overrides `observability.service_name` for exported controller telemetry. |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | Overrides the TOML base endpoint for all signals without a signal-specific endpoint. |
| `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`, `OTEL_EXPORTER_OTLP_METRICS_ENDPOINT`, `OTEL_EXPORTER_OTLP_LOGS_ENDPOINT` | Override the base endpoint for one signal. Each value is the complete signal URL. |
| `OTEL_EXPORTER_OTLP_HEADERS` | Supplies shared OTLP headers as comma-separated `name=value` pairs. |
| `OTEL_EXPORTER_OTLP_TRACES_HEADERS`, `OTEL_EXPORTER_OTLP_METRICS_HEADERS`, `OTEL_EXPORTER_OTLP_LOGS_HEADERS` | Replace shared headers for one signal rather than merging with them. |

Endpoint priority for each signal is:

1. Its signal-specific `OTEL_EXPORTER_OTLP_<SIGNAL>_ENDPOINT`.
2. `OTEL_EXPORTER_OTLP_ENDPOINT`.
3. `observability.otlp_endpoint`.

A base endpoint receives `/v1/traces`, `/v1/metrics`, or `/v1/logs`, preserving any existing path prefix. For example, `http://127.0.0.1:4318` becomes `http://127.0.0.1:4318/v1/traces` for traces. A signal-specific endpoint is already complete: include its full path, such as `https://otel.example.com/v1/traces`; no `/v1/<signal>` suffix is added.

Header values are percent-decoded after parsing the comma-separated pairs. Keep authorization values in the environment, for example `Authorization=Bearer%20TOKEN_VALUE`; do not put them in configuration files.

Telemetry export activates once after controller configuration is loaded. Restart the controller after changing export switches, endpoints, headers, service name, sampling, or `OTEL_SDK_DISABLED`. Saved query settings are used by subsequent queries and do not require a controller restart solely for a query change.

<a id="retention-shutdown-and-sampling"></a>

### Retention, shutdown, and sampling

`log_retention_days` applies only to controller-owned `controller.jsonl*` files. On telemetry activation, files whose modification time is older than the configured cutoff are pruned. It does not control Tempo, Loki, Prometheus, Grafana, evidence, or business-record retention. The development stack has its own seven-day backend retention.

At controller shutdown, the Worker relay and enabled providers are drained within the single `shutdown_timeout_secs` deadline. A stuck or unavailable backend cannot hold process exit indefinitely; pending telemetry can be lost. Retention and shutdown have dedicated TOML fields, not custom Wukong environment-variable overrides.

The controller uses the standard OpenTelemetry SDK sampler environment at activation. For example, `OTEL_TRACES_SAMPLER=parentbased_traceidratio` with `OTEL_TRACES_SAMPLER_ARG=0.1` retains all spans in sampled parent traces and approximately ten percent of new root traces. Use `always_on` or `always_off` when appropriate. Sampling reduces telemetry only; it never samples business execution, evidence, or recovery. Worker spans follow the propagated W3C sampling decision. Restart after changing sampling.

<a id="desktop-observability-view"></a>

## Desktop observability view

Open **Observability**, the fourth workspace view, for the selected run:

- **Local activity** reads the current run, Agents, ordered Events, tool states, job counts, and pending-evidence counts from the durable Store. It remains available when every external backend is absent or failing.
- **Traces**, **Logs**, and **Metrics** query Tempo, Loki, and Prometheus independently. An unconfigured, unauthorized, timed out, unavailable, rate-limited, oversized, or malformed backend response degrades its own panel without turning the other signals into failures.
- Trace search and detail retain run/trace correlation. Logs can be narrowed by trace ID when that correlation was exported.
- Metrics are explicitly **service-scoped**. Selecting a run validates the desktop context but does not turn a service metric into a per-run measurement. Use traces, logs, and durable local activity for run-level investigation.
- When `query.grafana_url` is configured, explicit Grafana actions open native-generated Explore links for traces, logs, or the selected metric. Links carry validated run/trace intent and the selected time range, not credentials. The local stack provisions the expected `tempo`, `loki`, and `prometheus` datasource identifiers.

A `partial`, `truncated`, or stale indication is a query/result limitation, not a business status. Follow continuation cursors, narrow the range, or consult original records instead of treating an empty or truncated external panel as proof of absence.

<a id="worker-trace-propagation-and-relay"></a>

## Worker trace propagation and relay

Linux Python Workers and native Windows Workers use standard W3C `traceparent` and optional `tracestate` context. Protocol 4 carries it in the top-level JSON-RPC `_wukong_telemetry` member, outside business `params`. Responses can use the same top-level member for base64-encoded OTLP `ExportTraceServiceRequest` and `ExportLogsServiceRequest` protobuf batches. Upgrade the controller and every Worker together: protocol-3 peers are rejected rather than silently using a compatibility path.

The side channel does not redefine JSON-RPC `id`, method, business parameters, request/effect hashes, stable request identity, receipts, idempotency, or replay decisions. Workers remove the request context before business dispatch; the controller removes response telemetry before normal result processing. Invalid or oversized telemetry leaves the original business message usable and can be dropped.

Workers keep completed spans in bounded process-local queues. They receive no OTLP backend destination or backend credentials and open no additional telemetry network connection. The only return path is the existing RPC response envelope. The controller owns the bounded relay queue, decodes and validates protobuf, applies explicit resource/span/log attribute allowlists, removes unsupported free-form content, and exports with controller-owned endpoints and headers.

Telemetry queues are lossy under overflow, malformed input, disabled signals, shutdown, or backend failure. The controller relay does not delay a Worker business response while exporting. When metrics export is active, `wukong_telemetry_dropped_total` reports dropped telemetry by signal and bounded reason; its absence does not prove complete delivery.

<a id="local-acceptance-stack"></a>

## Local acceptance stack

`scripts/observability/compose.yaml` provides an optional **controller-only development acceptance stack**. It is not a production deployment and is not connected to challenge or tool networks. It has a dedicated Compose bridge with outbound masquerading disabled, loopback-only host ports, no Docker socket mount, and no credentials. Docker's `internal` network mode is not used because it does not publish controller-accessible host ports on Docker Desktop. Grafana anonymous Viewer access is therefore intended only for local development.

<a id="fixed-images-and-boundaries"></a>

### Fixed images and boundaries

| Component | Pinned image |
| --- | --- |
| OpenTelemetry Collector | `otel/opentelemetry-collector:0.128.0` |
| Tempo | `grafana/tempo:2.8.1` |
| Loki | `grafana/loki:3.5.1` |
| Prometheus | `prom/prometheus:v3.4.1` |
| Grafana | `grafana/grafana:12.0.1` |

Tempo, Loki, and Prometheus retain local telemetry for seven days. Their data and Grafana state use named volumes in the `wukong-observability` Compose project. This retention is independent of controller JSONL retention.

<a id="start-health-and-stop"></a>

### Start, check health, and stop

Run these commands from the repository root. Parsing and starting can pull the pinned images; these commands do not start challenge or tool containers.

```sh
docker compose -f scripts/observability/compose.yaml config
docker compose -f scripts/observability/compose.yaml up -d --wait
docker compose -f scripts/observability/compose.yaml ps
docker compose -f scripts/observability/compose.yaml logs --tail=100 collector
```

`up --wait` and `ps` use the Compose health checks. The equivalent host-side checks are:

```sh
curl --fail http://127.0.0.1:13133/
curl --fail http://127.0.0.1:3200/ready
curl --fail http://127.0.0.1:3100/ready
curl --fail http://127.0.0.1:9090/-/ready
curl --fail http://127.0.0.1:3000/api/health
```

The Collector container health check validates its loaded configuration; the host-side port `13133` request checks its live health endpoint. Configure the controller base endpoint as `http://127.0.0.1:4318`, using the example above. Open Grafana at <http://127.0.0.1:3000>; the fixed **Wukong Observability** dashboard provides Run ID and Trace ID filters, trace search/detail, logs, metric rates, and Explore links.

Stop only this Compose project with:

```sh
docker compose -f scripts/observability/compose.yaml down
```

`down` retains its named volumes. Do not add `--volumes` unless explicitly intending to delete this stack's local telemetry data.

<a id="acceptance-procedure"></a>

### Acceptance procedure

The following is a procedure to execute, **not a claim that real acceptance has passed**:

1. Run the Compose parsing/start/health commands above and inspect any failed service before testing export.
2. Apply the example configuration, restart the controller, and execute an identified analysis. Check durable Local activity first, then trace search/detail, curated logs when enabled, service-scoped metrics, and each Grafana deep link.
3. Separately exercise configured, isolated Linux and Windows Worker environments. Confirm linked Worker spans where sampling permits and verify that business request IDs, results, evidence hashes, retries, and recovery behavior are unchanged.
4. Stop or misconfigure one query backend and verify that only its external panel degrades while Local activity and the other external panels remain usable. Restore it before continuing.
5. Inspect dropped-telemetry behavior under sampling, queue pressure, or backend interruption without interpreting missing telemetry as missing business work.
6. Stop the stack with `down`, retaining volumes unless deletion was explicitly requested.

Static validation is available with:

```sh
python -m unittest tests.automation.test_observability -v
```

With the stack running, a bounded synthetic OTLP/query precheck is available:

```sh
python -I scripts/observability/acceptance.py
```

It prints a run/trace pair for Grafana inspection and does not replace the controller, Worker, or GUI procedure above.

Configuration parsing and static tests do not replace real OTLP export, backend query, dashboard, native GUI, live-model, or Linux/Windows Worker acceptance. Record the actual target, configuration, package identities, executed steps, and uncovered areas separately.

<a id="query-limits-and-troubleshooting"></a>

## Query limits and troubleshooting

<a id="pagination-and-bounds"></a>

### Pagination and bounds

- Query pages accept limits from 1 through 500. Trace/log cursors belong to their original time range; Local activity uses a nonnegative event-sequence cursor. Continue only with the returned cursor and keep the range and filters consistent.
- The configured maximum range is 60 seconds through 31 days and defaults to one day. Narrow large ranges instead of repeatedly increasing timeouts.
- Local activity returns at most 256 Agents, omits oversized event payloads with explicit truncation markers, and bounds the event timeline to 1 MiB. A returned `next_cursor`/`has_more` means more local events exist.
- Trace detail and log/metric mapping are bounded. Logs display at most 4096 characters per line; metrics retain at most 32 series and apply the requested point limit. Backend JSON responses have a 2 MiB default cap.
- `truncated` can mean omitted pages, spans, series, points, Agents, event payloads, or long log lines. It never changes the underlying durable record or backend data.

<a id="common-failures"></a>

### Common failures

| Symptom | Checks |
| --- | --- |
| Every external panel is unconfigured, but Local activity works | Configure `query.tempo`, `query.loki`, and `query.prometheus` separately. Export configuration alone does not configure queries. |
| Traces or metrics are absent | Configure an OTLP endpoint, leave the corresponding signal enabled, ensure `OTEL_SDK_DISABLED` is not `true`, restart, and check the selected time range and sampler. |
| Logs are absent | Set `logs = true`, configure the Loki query URL and OTLP logs destination, restart, and check trace/run correlation and backend retention. Local JSONL is a separate sink. |
| One panel reports authentication failure | Set the environment variable named by that backend's `credential_env` to a valid bearer token and retry. Do not paste the token into settings. |
| A remote HTTP endpoint is rejected | Use HTTPS. Plain HTTP is accepted only for `localhost`, `127.0.0.1`, or `::1`; remove userinfo, query strings, and fragments. |
| Export changes have no effect | Restart the controller. Query-only changes are used by subsequent queries after being saved. |
| Results are partial or truncated | Follow the cursor, reduce the time range/page size, and inspect original Store records or backend data. A filtered query response is not authoritative evidence. |
| Telemetry disappears during overload or shutdown | Expect best-effort loss from local writer, Worker, or controller queues. Check `wukong_telemetry_dropped_total` when available, Collector/backend health, and the configured shutdown deadline; never rerun uncertain business effects merely to regenerate telemetry. |
| Grafana opens but shows no data | Verify backend health and datasource identifiers, the selected range/run/trace filters, signal export, and retention. A generated deep link does not authenticate a backend or prove that matching telemetry exists. |

For business completion, cleanup, evidence, and recovery questions, return to the durable workspace and original evidence rather than inferring state from these operational views.
