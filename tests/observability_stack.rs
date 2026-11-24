//! Real local-stack acceptance for the same native query facade used by Tauri.
//! Run `python -I scripts/observability/acceptance.py` first and pass its IDs via
//! `WUKONG_OBSERVABILITY_ACCEPTANCE_RUN` and `WUKONG_OBSERVABILITY_ACCEPTANCE_TRACE`.

use chrono::{Duration, Utc};
use opentelemetry::trace::TraceContextExt as _;
use tracing::Instrument as _;
use tracing_opentelemetry::OpenTelemetrySpanExt as _;
use wukong::adapters::{
    observability::{GrafanaTarget, MetricName, Observability, PageRequest, TimeRange},
    telemetry::{BackendConfig, ObservabilityConfig, QueryConfig, TelemetryRuntime},
    worker::{DockerConfig, RuntimeClient, RuntimeConfig, TransportConfig, TrustedLocalConfig},
};

fn backend(url: &str) -> Option<BackendConfig> {
    Some(BackendConfig {
        url: url.into(),
        credential_env: None,
    })
}

fn controller_config(state_dir: &std::path::Path) -> wukong::config::AppConfig {
    wukong::config::AppConfig {
        state_dir: state_dir.to_path_buf(),
        observability: ObservabilityConfig {
            service_name: "wukong".into(),
            otlp_endpoint: Some("http://127.0.0.1:4318".into()),
            traces: true,
            metrics: true,
            logs: true,
            local_jsonl: false,
            log_retention_days: 7,
            shutdown_timeout_secs: 10,
            query: QueryConfig {
                tempo: backend("http://127.0.0.1:3200"),
                loki: backend("http://127.0.0.1:3100"),
                prometheus: backend("http://127.0.0.1:9090"),
                grafana_url: Some("http://127.0.0.1:3000".into()),
                timeout_secs: 5,
                max_range_secs: 86_400,
            },
        },
        ..Default::default()
    }
}

#[tokio::test]
#[ignore = "requires the running loopback observability stack and acceptance IDs"]
async fn native_facade_reads_correlated_real_backend_data() {
    let run = std::env::var("WUKONG_OBSERVABILITY_ACCEPTANCE_RUN")
        .expect("acceptance run ID from scripts/observability/acceptance.py");
    let trace_id = std::env::var("WUKONG_OBSERVABILITY_ACCEPTANCE_TRACE")
        .expect("acceptance trace ID from scripts/observability/acceptance.py");
    let observability = Observability::new(QueryConfig {
        tempo: backend("http://127.0.0.1:3200"),
        loki: backend("http://127.0.0.1:3100"),
        prometheus: backend("http://127.0.0.1:9090"),
        grafana_url: Some("http://127.0.0.1:3000".into()),
        timeout_secs: 5,
        max_range_secs: 86_400,
    })
    .expect("valid loopback backend configuration");
    let range = TimeRange {
        start: Utc::now() - Duration::minutes(15),
        end: Utc::now() + Duration::minutes(5),
    };
    let page = PageRequest {
        range: range.clone(),
        limit: 20,
        cursor: None,
    };

    let traces = observability
        .search_traces(&run, page.clone())
        .await
        .expect("valid trace query");
    assert!(traces.configured && !traces.partial, "{:?}", traces.error);
    let traces = traces.data.expect("Tempo data");
    assert!(traces.traces.iter().any(|trace| trace.trace_id == trace_id));

    let trace = observability
        .trace(&run, &trace_id)
        .await
        .expect("valid trace detail query");
    assert!(trace.configured && !trace.partial, "{:?}", trace.error);
    let trace = trace.data.expect("Tempo trace detail");
    assert_eq!(trace.trace_id, trace_id);
    assert!(trace.spans.iter().any(|span| {
        span.attributes
            .get("run_id")
            .and_then(serde_json::Value::as_str)
            == Some(run.as_str())
    }));

    let logs = observability
        .logs(&run, Some(&trace_id), page)
        .await
        .expect("valid log query");
    assert!(logs.configured && !logs.partial, "{:?}", logs.error);
    let logs = logs.data.expect("Loki entries");
    assert!(!logs.entries.is_empty());

    let metrics = observability
        .metrics(run.clone(), MetricName::RunsStarted, range.clone(), 20)
        .await
        .expect("valid metric query");
    assert!(
        metrics.configured && !metrics.partial,
        "{:?}",
        metrics.error
    );
    let metrics = metrics.data.expect("Prometheus series");
    assert!(
        metrics
            .series
            .iter()
            .any(|series| !series.points.is_empty())
    );

    let link = observability
        .grafana_link(GrafanaTarget::Traces { run }, range)
        .expect("valid Grafana target")
        .expect("configured Grafana URL");
    assert!(link.starts_with("http://127.0.0.1:3000/explore?"));
}

#[tokio::test]
#[ignore = "requires the running loopback observability stack"]
async fn controller_runtime_exports_all_signals_to_real_backends() {
    let run = format!("observability-runtime-{}", Utc::now().timestamp_millis());
    let state = tempfile::tempdir().expect("temporary controller state");
    let config = controller_config(state.path());

    let runtime = TelemetryRuntime::new();
    runtime.activate(&config);
    let status = runtime.handle().status();
    assert!(
        status.activated && status.traces && status.metrics && status.logs,
        "{status:?}"
    );

    let metrics = runtime.handle().metrics().clone();
    let active = metrics.run_started();
    let span = tracing::info_span!(target: "wukong", "wukong.assessment", run_id = %run);
    let trace_id = span.in_scope(|| {
        let trace_id = span.context().span().span_context().trace_id().to_string();
        tracing::info!(
            target: "wukong_telemetry",
            event = "run_finished",
            run_id = %run,
            trace_id = %trace_id,
            outcome = "completed",
        );
        trace_id
    });
    assert_ne!(trace_id, "00000000000000000000000000000000");
    metrics.run_finished("completed", 0.001);
    drop(active);
    drop(span);
    runtime.shutdown().await;

    let observability = Observability::new(config.observability.query)
        .expect("valid loopback backend configuration");
    let range = TimeRange {
        start: Utc::now() - Duration::minutes(15),
        end: Utc::now() + Duration::minutes(5),
    };
    let page = PageRequest {
        range: range.clone(),
        limit: 20,
        cursor: None,
    };

    let mut found_trace = false;
    for _ in 0..45 {
        let traces = observability
            .search_traces(&run, page.clone())
            .await
            .expect("valid trace query");
        if traces
            .data
            .is_some_and(|data| data.traces.iter().any(|trace| trace.trace_id == trace_id))
        {
            found_trace = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    assert!(found_trace, "controller trace was not indexed by Tempo");
    let trace = observability
        .trace(&run, &trace_id)
        .await
        .expect("valid trace detail query")
        .data
        .expect("Tempo trace detail");
    assert!(trace.spans.iter().any(|span| {
        span.name == "wukong.assessment"
            && span
                .attributes
                .get("run_id")
                .and_then(serde_json::Value::as_str)
                == Some(run.as_str())
    }));

    let mut found_log = false;
    for _ in 0..45 {
        let logs = observability
            .logs(&run, Some(&trace_id), page.clone())
            .await
            .expect("valid log query");
        if logs.data.is_some_and(|data| !data.entries.is_empty()) {
            found_log = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    assert!(found_log, "controller curated log was not indexed by Loki");

    let mut found_metric = false;
    for _ in 0..60 {
        let metrics = observability
            .metrics(run.clone(), MetricName::RunsStarted, range.clone(), 20)
            .await
            .expect("valid metric query");
        if metrics
            .data
            .is_some_and(|data| data.series.iter().any(|series| !series.points.is_empty()))
        {
            found_metric = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    assert!(
        found_metric,
        "controller metric was not indexed by Prometheus"
    );
}

#[tokio::test]
#[ignore = "requires the loopback stack and an isolated current-protocol Docker Worker"]
async fn docker_worker_relay_exports_correlated_spans_to_tempo() {
    let container = std::env::var("WUKONG_OBSERVABILITY_WORKER_CONTAINER")
        .expect("isolated Docker Worker container name");
    assert_worker_relay(RuntimeConfig {
        transport: TransportConfig::Docker(DockerConfig::new(container)),
        request_timeout_secs: 30,
    })
    .await;
}

#[tokio::test]
#[ignore = "requires the loopback stack and trusted local Python Worker fixtures"]
async fn trusted_local_worker_relay_exports_correlated_spans_to_tempo() {
    let worker = tempfile::tempdir().expect("temporary Worker state");
    assert_worker_relay(RuntimeConfig {
        transport: TransportConfig::TrustedLocal(TrustedLocalConfig {
            state_dir: worker.path().join("state"),
            python: "python".into(),
            bridge_path: std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("workers/runtime/bridge.py"),
        }),
        request_timeout_secs: 30,
    })
    .await;
}

async fn assert_worker_relay(runtime_config: RuntimeConfig) {
    let run = format!("observability-worker-{}", Utc::now().timestamp_millis());
    let state = tempfile::tempdir().expect("temporary controller state");
    let config = controller_config(state.path());
    let runtime = TelemetryRuntime::new();
    runtime.activate(&config);
    let client = RuntimeClient::connect_with_telemetry(runtime_config, runtime.handle())
        .await
        .expect("connect to the real Worker");

    let span = tracing::info_span!(target: "wukong", "wukong.assessment", run_id = %run);
    let trace_id = span.in_scope(|| span.context().span().span_context().trace_id().to_string());
    client
        .doctor()
        .instrument(span.clone())
        .await
        .expect("Worker doctor succeeds with telemetry attached");
    drop(span);
    client.close().await.expect("close the Worker transport");
    runtime.shutdown().await;

    let observability = Observability::new(config.observability.query)
        .expect("valid loopback backend configuration");
    let mut correlated = false;
    for _ in 0..45 {
        let signal = observability
            .trace(&run, &trace_id)
            .await
            .expect("valid trace detail query");
        correlated = signal.data.is_some_and(|trace| {
            let controller_rpc = trace
                .spans
                .iter()
                .find(|span| span.name == "wukong.worker.rpc");
            let worker_rpc = trace.spans.iter().find(|span| {
                span.name == "rpc.server"
                    && span
                        .resource
                        .get("service.name")
                        .and_then(|value| value.as_str())
                        == Some("wukong-worker")
            });
            match (controller_rpc, worker_rpc) {
                (Some(controller), Some(worker)) => {
                    worker.parent_span_id.as_deref() == Some(controller.span_id.as_str())
                }
                _ => false,
            }
        });
        if correlated {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    assert!(
        correlated,
        "relayed Worker span was not indexed beneath the controller RPC span"
    );
}
