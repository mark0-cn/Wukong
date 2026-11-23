# Local OpenTelemetry acceptance stack

English · [简体中文](README.zh-CN.md)

This directory contains a controller-only development stack. It receives OTLP
HTTP from the host and exports traces to Tempo, logs to Loki, and metrics to
Prometheus. Grafana provides a fixed dashboard and links to Explore. The stack
has a dedicated Compose bridge with outbound masquerading disabled, no Docker
socket mount, no challenge/tool network attachment, and no credentials. Host
ports bind only to loopback because Docker's `internal` network mode does not
publish controller-accessible host ports on Docker Desktop. Anonymous Grafana
access is intentionally limited to local development by those loopback ports.

## Fixed images

| Component | Image |
| --- | --- |
| OpenTelemetry Collector | `otel/opentelemetry-collector:0.128.0` |
| Tempo | `grafana/tempo:2.8.1` |
| Loki | `grafana/loki:3.5.1` |
| Prometheus | `prom/prometheus:v3.4.1` |
| Grafana | `grafana/grafana:12.0.1` |

Tempo, Loki, and Prometheus retain local data for seven days. Their state and
Grafana state use only the `wukong-observability` Compose project's named volumes.

## Start, inspect, and stop

Run these commands from the repository root. Parsing and starting may pull the
pinned images; no command starts challenge or tool containers.

```sh
docker compose -f scripts/observability/compose.yaml config
docker compose -f scripts/observability/compose.yaml up -d --wait
docker compose -f scripts/observability/compose.yaml ps
docker compose -f scripts/observability/compose.yaml logs --tail=100 collector
docker compose -f scripts/observability/compose.yaml down
```

`down` stops only this Compose project and retains its named volumes. Do not add
`--volumes` unless the task explicitly requires deleting this stack's local data.

Configure the controller's standard OTLP base endpoint as
`http://127.0.0.1:4318`; the controller appends the signal-specific paths. Open
Grafana at <http://127.0.0.1:3000>. The fixed **Wukong Observability** dashboard
provides Run ID and Trace ID filters, trace search, trace detail, logs, metric
rates, and an Explore link. Native read-only query adapters use:

- Tempo: `http://127.0.0.1:3200`
- Loki: `http://127.0.0.1:3100`
- Prometheus: `http://127.0.0.1:9090`

## Health checks

`up --wait` and `ps` use the Compose health checks. The corresponding host-side
checks are:

```sh
curl --fail http://127.0.0.1:13133/
curl --fail http://127.0.0.1:3200/ready
curl --fail http://127.0.0.1:3100/ready
curl --fail http://127.0.0.1:9090/-/ready
curl --fail http://127.0.0.1:3000/api/health
```

The Collector container health check validates its loaded configuration; use the
host-side `13133` request to check its live health endpoint.

Run static validation with:

```sh
python -m unittest tests.automation.test_observability -v
```

After `up --wait` succeeds, exercise real OTLP ingestion and the Tempo, Loki,
Prometheus, and Grafana query/provisioning APIs with a fixed synthetic run:

```sh
python -I scripts/observability/acceptance.py
```

The probe prints its run and trace IDs for inspection in Grafana. It does not
replace a real controller model analysis, Worker relay, native GUI, or platform
acceptance. Configuration parsing and static tests do not replace real OTLP
export, backend query, dashboard, or native GUI acceptance.

Exercise the actual controller-owned `TelemetryRuntime` and verify its traces,
curated logs, and metrics through the native query facade with:

```sh
cargo test --test observability_stack controller_runtime_exports_all_signals_to_real_backends -- --ignored --test-threads=1
```

On Windows, use the repository's validated MSVC entry point when Cargo needs it:

```powershell
python -m scripts.dev.msvc_cargo -- test --test observability_stack controller_runtime_exports_all_signals_to_real_backends -- --ignored --test-threads=1
```

The browser workspace acceptance uses real ECharts and vis-timeline rendering
with a simulated Tauri IPC boundary. Install Playwright Chromium if necessary,
then run `npm run test:e2e -- observability.spec.ts`. It is not native Tauri
acceptance and does not query the live backends.

With an isolated Docker Worker built from the same source, set
`WUKONG_OBSERVABILITY_WORKER_CONTAINER` to its container name and run:

```sh
cargo test --test observability_stack docker_worker_relay_exports_correlated_spans_to_tempo -- --ignored --test-threads=1
```

On Windows, use `python -m scripts.dev.msvc_cargo -- test ...` with the same test
arguments. This verifies the real stdio response relay and requires the exported
Worker `rpc.server` span to be a child of the controller `wukong.worker.rpc`
span in Tempo. The Worker retains its `none` network and receives no backend
endpoint or credential.

The same relay and parentage assertion can be exercised with the trusted local
Python Worker development fixture, without Docker:

```sh
cargo test --test observability_stack trusted_local_worker_relay_exports_correlated_spans_to_tempo -- --ignored --test-threads=1
```

This trusted fixture is only for the checked-out Worker source; it is not a
fallback for unknown challenges or a substitute for Docker platform acceptance.
