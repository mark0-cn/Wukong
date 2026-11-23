//! Low-cardinality operational instruments. Business identifiers stay on traces/logs.

use opentelemetry::{
    KeyValue, global,
    metrics::{Counter, Histogram, UpDownCounter},
};

#[derive(Clone)]
pub struct Metrics {
    runs_started: Counter<u64>,
    runs_finished: Counter<u64>,
    active_runs: UpDownCounter<i64>,
    run_duration: Histogram<f64>,
    generation_duration: Histogram<f64>,
    worker_rpc_duration: Histogram<f64>,
    worker_rpc_retries: Counter<u64>,
    model_duration: Histogram<f64>,
    token_usage: Histogram<u64>,
    evidence_sync_duration: Histogram<f64>,
    telemetry_dropped: Counter<u64>,
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

impl Metrics {
    pub fn new() -> Self {
        let meter = global::meter("wukong");
        Self {
            runs_started: meter.u64_counter("wukong.run.started").build(),
            runs_finished: meter.u64_counter("wukong.run.finished").build(),
            active_runs: meter.i64_up_down_counter("wukong.run.active").build(),
            run_duration: meter
                .f64_histogram("wukong.run.duration")
                .with_unit("s")
                .build(),
            generation_duration: meter
                .f64_histogram("wukong.agent.generation.duration")
                .with_unit("s")
                .build(),
            worker_rpc_duration: meter
                .f64_histogram("wukong.worker.rpc.duration")
                .with_unit("s")
                .build(),
            worker_rpc_retries: meter.u64_counter("wukong.worker.rpc.retries").build(),
            model_duration: meter
                .f64_histogram("gen_ai.client.operation.duration")
                .with_unit("s")
                .build(),
            token_usage: meter
                .u64_histogram("gen_ai.client.token.usage")
                .with_unit("{token}")
                .build(),
            evidence_sync_duration: meter
                .f64_histogram("wukong.evidence.sync.duration")
                .with_unit("s")
                .build(),
            telemetry_dropped: meter.u64_counter("wukong.telemetry.dropped").build(),
        }
    }

    pub fn run_started(&self) -> RunMetricGuard {
        self.runs_started.add(1, &[]);
        self.active_runs.add(1, &[]);
        RunMetricGuard {
            active: self.active_runs.clone(),
        }
    }

    pub fn run_finished(&self, outcome: &str, seconds: f64) {
        let attributes = [KeyValue::new("outcome", outcome.to_owned())];
        self.runs_finished.add(1, &attributes);
        self.run_duration.record(seconds, &attributes);
    }

    pub fn generation(&self, outcome: &str, seconds: f64) {
        self.generation_duration
            .record(seconds, &[KeyValue::new("outcome", outcome.to_owned())]);
    }

    pub fn worker_rpc(
        &self,
        method: &str,
        transport: &str,
        outcome: &str,
        retries: u64,
        seconds: f64,
    ) {
        let attributes = [
            KeyValue::new("rpc.method", method.to_owned()),
            KeyValue::new("transport", transport.to_owned()),
            KeyValue::new("outcome", outcome.to_owned()),
        ];
        self.worker_rpc_duration.record(seconds, &attributes);
        if retries > 0 {
            self.worker_rpc_retries.add(retries, &attributes);
        }
    }

    pub fn model_operation(
        &self,
        provider: &str,
        outcome: &str,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        seconds: f64,
    ) {
        self.model_duration.record(
            seconds,
            &[
                KeyValue::new("gen_ai.provider.name", provider.to_owned()),
                KeyValue::new("outcome", outcome.to_owned()),
            ],
        );
        for (kind, value) in [("input", input_tokens), ("output", output_tokens)] {
            if let Some(value) = value {
                self.token_usage.record(
                    value,
                    &[
                        KeyValue::new("gen_ai.provider.name", provider.to_owned()),
                        KeyValue::new("gen_ai.token.type", kind),
                    ],
                );
            }
        }
    }

    pub fn evidence_sync(&self, outcome: &str, seconds: f64) {
        self.evidence_sync_duration
            .record(seconds, &[KeyValue::new("outcome", outcome.to_owned())]);
    }

    pub fn dropped(&self, signal: &str, reason: &str, count: u64) {
        self.telemetry_dropped.add(
            count,
            &[
                KeyValue::new("signal", signal.to_owned()),
                KeyValue::new("reason", reason.to_owned()),
            ],
        );
    }
}

pub struct RunMetricGuard {
    active: UpDownCounter<i64>,
}

impl Drop for RunMetricGuard {
    fn drop(&mut self) {
        self.active.add(-1, &[]);
    }
}
