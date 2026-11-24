//! Public dynamic-tracing specifications, stable logical plans, and raw event documents.
use anyhow::ensure;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const TRACE_PLAN_DOMAIN: &[u8] = b"wukong.trace.plan.v1\0";
const MAX_CONSTANT_BUFFER_BYTES: u64 = 1024 * 1024;

fn default_schema_version() -> u32 {
    1
}
fn default_max_events() -> u64 {
    1024
}
fn default_max_point_hits() -> u64 {
    256
}
fn default_max_buffer_bytes() -> u64 {
    4096
}
fn default_max_event_bytes() -> u64 {
    65536
}
fn default_max_events_bytes() -> u64 {
    8 * 1024 * 1024
}
fn default_stop_wait_ms() -> u64 {
    10000
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TraceAbi {
    #[default]
    SysvAmd64,
}

#[derive(
    Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum FunctionPhase {
    Entry,
    Return,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IntegerType {
    U8,
    U16,
    U32,
    U64,
    I8,
    I16,
    I32,
    I64,
    Pointer,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TraceLocation {
    Symbol {
        #[schemars(regex(pattern = "^[0-9a-f]{64}$"))]
        module_sha256: String,
        #[schemars(length(min = 1), regex(pattern = "^[A-Za-z_][A-Za-z0-9_]*$"))]
        symbol: String,
    },
    ModuleOffset {
        #[schemars(regex(pattern = "^[0-9a-f]{64}$"))]
        module_sha256: String,
        offset: u64,
    },
}

impl TraceLocation {
    fn validate(&self) -> anyhow::Result<()> {
        let module_sha256 = match self {
            Self::Symbol {
                module_sha256,
                symbol,
            } => {
                validate_symbol(symbol)?;
                module_sha256
            }
            Self::ModuleOffset { module_sha256, .. } => module_sha256,
        };
        validate_module_sha256(module_sha256)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ArgumentCapture {
    #[schemars(length(min = 1, max = 64))]
    pub name: String,
    #[schemars(range(min = 0, max = 5))]
    pub index: u8,
    pub value_type: IntegerType,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BufferLength {
    Constant {
        #[schemars(range(min = 0, max = 1048576))]
        bytes: u64,
    },
    Argument {
        #[schemars(range(min = 0, max = 5))]
        index: u8,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BufferCapture {
    #[schemars(length(min = 1, max = 64))]
    pub name: String,
    #[schemars(range(min = 0, max = 5))]
    pub address_argument: u8,
    pub length: BufferLength,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TraceCapture {
    #[serde(default)]
    pub arguments: Vec<ArgumentCapture>,
    #[serde(default)]
    pub buffers: Vec<BufferCapture>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub return_value: Option<IntegerType>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TracePoint {
    Function {
        #[schemars(
            length(min = 1, max = 64),
            regex(pattern = "^[A-Za-z][A-Za-z0-9_.-]{0,63}$")
        )]
        point_id: String,
        location: TraceLocation,
        #[schemars(length(min = 1, max = 2))]
        phases: Vec<FunctionPhase>,
        capture: TraceCapture,
    },
    CallSite {
        #[schemars(
            length(min = 1, max = 64),
            regex(pattern = "^[A-Za-z][A-Za-z0-9_.-]{0,63}$")
        )]
        point_id: String,
        location: TraceLocation,
        capture: TraceCapture,
    },
}

impl TracePoint {
    pub fn point_id(&self) -> &str {
        match self {
            Self::Function { point_id, .. } | Self::CallSite { point_id, .. } => point_id,
        }
    }

    pub fn location(&self) -> &TraceLocation {
        match self {
            Self::Function { location, .. } | Self::CallSite { location, .. } => location,
        }
    }

    pub fn capture(&self) -> &TraceCapture {
        match self {
            Self::Function { capture, .. } | Self::CallSite { capture, .. } => capture,
        }
    }

    fn validate(&self) -> anyhow::Result<()> {
        validate_identifier(self.point_id())?;
        self.location().validate()?;
        let capture = self.capture();
        if let Self::Function { phases, .. } = self {
            ensure!(
                !phases.is_empty(),
                "trace function phases must not be empty"
            );
            let unique: BTreeSet<_> = phases.iter().collect();
            ensure!(
                unique.len() == phases.len(),
                "duplicate trace function phase"
            );
            if capture.return_value.is_some() {
                ensure!(
                    phases.contains(&FunctionPhase::Return),
                    "trace return_value requires a return phase"
                );
            }
        } else {
            ensure!(
                capture.return_value.is_none(),
                "trace return_value is only valid for return functions"
            );
        }

        let mut names = BTreeSet::new();
        let mut argument_indices = BTreeSet::new();
        for argument in &capture.arguments {
            validate_capture_name(&argument.name)?;
            ensure!(names.insert(&argument.name), "duplicate trace capture name");
            ensure!(argument.index <= 5, "trace argument index exceeds 5");
            ensure!(
                argument_indices.insert(argument.index),
                "duplicate trace argument index"
            );
        }
        for buffer in &capture.buffers {
            validate_capture_name(&buffer.name)?;
            ensure!(names.insert(&buffer.name), "duplicate trace capture name");
            ensure!(
                buffer.address_argument <= 5,
                "trace buffer address index exceeds 5"
            );
            match buffer.length {
                BufferLength::Constant { bytes } => ensure!(
                    bytes <= MAX_CONSTANT_BUFFER_BYTES,
                    "trace constant buffer length exceeds 1 MiB"
                ),
                BufferLength::Argument { index } => {
                    ensure!(index <= 5, "trace buffer length index exceeds 5")
                }
            }
        }
        Ok(())
    }

    fn logical_plan(&self) -> Value {
        let point_id = self.point_id().to_owned();
        let capture = canonical_capture(self.capture());
        match self {
            Self::Function { phases, .. } => {
                let mut phases = phases.clone();
                phases.sort_unstable();
                canonical_object([
                    ("capture", capture),
                    ("kind", Value::from("function")),
                    (
                        "phases",
                        serde_json::to_value(phases).expect("trace phases serialize"),
                    ),
                    ("point_id", Value::from(point_id)),
                ])
            }
            Self::CallSite { .. } => canonical_object([
                ("capture", capture),
                ("kind", Value::from("call_site")),
                ("point_id", Value::from(point_id)),
            ]),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TraceLimits {
    #[serde(default = "default_max_events")]
    #[schemars(range(min = 1, max = 4096))]
    pub max_events: u64,
    #[serde(default = "default_max_point_hits")]
    #[schemars(range(min = 1, max = 1024))]
    pub max_point_hits: u64,
    #[serde(default = "default_max_buffer_bytes")]
    #[schemars(range(min = 1, max = 65536))]
    pub max_buffer_bytes: u64,
    #[serde(default = "default_max_event_bytes")]
    #[schemars(range(min = 1024, max = 262144))]
    pub max_event_bytes: u64,
    #[serde(default = "default_max_events_bytes")]
    #[schemars(range(min = 4096, max = 16777216))]
    pub max_events_bytes: u64,
    #[serde(default = "default_stop_wait_ms")]
    #[schemars(range(min = 100, max = 30000))]
    pub stop_wait_ms: u64,
}

impl Default for TraceLimits {
    fn default() -> Self {
        Self {
            max_events: default_max_events(),
            max_point_hits: default_max_point_hits(),
            max_buffer_bytes: default_max_buffer_bytes(),
            max_event_bytes: default_max_event_bytes(),
            max_events_bytes: default_max_events_bytes(),
            stop_wait_ms: default_stop_wait_ms(),
        }
    }
}

impl TraceLimits {
    fn validate(&self) -> anyhow::Result<()> {
        ensure!(
            (1..=4096).contains(&self.max_events),
            "invalid trace max_events"
        );
        ensure!(
            (1..=1024).contains(&self.max_point_hits),
            "invalid trace max_point_hits"
        );
        ensure!(
            (1..=65536).contains(&self.max_buffer_bytes),
            "invalid trace max_buffer_bytes"
        );
        ensure!(
            (1024..=262144).contains(&self.max_event_bytes),
            "invalid trace max_event_bytes"
        );
        ensure!(
            (4096..=16 * 1024 * 1024).contains(&self.max_events_bytes),
            "invalid trace max_events_bytes"
        );
        ensure!(
            (100..=30000).contains(&self.stop_wait_ms),
            "invalid trace stop_wait_ms"
        );
        ensure!(
            self.max_event_bytes <= self.max_events_bytes,
            "trace max_event_bytes exceeds max_events_bytes"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TraceSpec {
    #[serde(default = "default_schema_version")]
    #[schemars(range(min = 1, max = 1))]
    pub schema_version: u32,
    #[serde(default)]
    pub abi: TraceAbi,
    #[schemars(length(min = 1, max = 64))]
    pub points: Vec<TracePoint>,
    #[serde(default)]
    pub limits: TraceLimits,
}

impl TraceSpec {
    pub fn validate(&self) -> anyhow::Result<()> {
        ensure!(self.schema_version == 1, "unsupported trace schema_version");
        ensure!(
            !self.points.is_empty() && self.points.len() <= 64,
            "trace points must contain 1..64 entries"
        );
        self.limits.validate()?;
        let mut point_ids = BTreeSet::new();
        for point in &self.points {
            point.validate()?;
            ensure!(
                point_ids.insert(point.point_id()),
                "duplicate trace point_id"
            );
        }
        Ok(())
    }

    /// Canonical semantics used by both controller and worker; locations and limits are excluded.
    pub fn logical_plan(&self) -> Value {
        let mut points = self.points.clone();
        points.sort_unstable_by(|left, right| left.point_id().cmp(right.point_id()));
        canonical_object([
            (
                "abi",
                serde_json::to_value(self.abi).expect("trace ABI serializes"),
            ),
            (
                "points",
                Value::Array(points.iter().map(TracePoint::logical_plan).collect()),
            ),
            ("schema_version", Value::from(self.schema_version)),
        ])
    }

    pub fn plan_signature(&self) -> String {
        let canonical = serde_json::to_vec(&self.logical_plan()).expect("trace plan serializes");
        let mut digest = Sha256::new();
        digest.update(TRACE_PLAN_DOMAIN);
        digest.update(canonical);
        format!("{:x}", digest.finalize())
    }
}

fn canonical_object<const N: usize>(entries: [(&str, Value); N]) -> Value {
    let entries: BTreeMap<String, Value> = entries
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect();
    serde_json::to_value(entries).expect("canonical trace object serializes")
}

fn canonical_capture(capture: &TraceCapture) -> Value {
    let mut arguments = capture.arguments.clone();
    arguments.sort_unstable_by(|left, right| left.name.cmp(&right.name));
    let mut buffers = capture.buffers.clone();
    buffers.sort_unstable_by(|left, right| left.name.cmp(&right.name));
    let arguments = arguments
        .iter()
        .map(|argument| {
            canonical_object([
                ("index", Value::from(argument.index)),
                ("name", Value::from(argument.name.clone())),
                (
                    "value_type",
                    serde_json::to_value(argument.value_type).expect("integer type serializes"),
                ),
            ])
        })
        .collect();
    let buffers = buffers
        .iter()
        .map(|buffer| {
            let length = match buffer.length {
                BufferLength::Constant { bytes } => canonical_object([
                    ("bytes", Value::from(bytes)),
                    ("kind", Value::from("constant")),
                ]),
                BufferLength::Argument { index } => canonical_object([
                    ("index", Value::from(index)),
                    ("kind", Value::from("argument")),
                ]),
            };
            canonical_object([
                ("address_argument", Value::from(buffer.address_argument)),
                ("length", length),
                ("name", Value::from(buffer.name.clone())),
            ])
        })
        .collect();
    let mut fields = BTreeMap::from([
        ("arguments".to_owned(), Value::Array(arguments)),
        ("buffers".to_owned(), Value::Array(buffers)),
    ]);
    if let Some(return_value) = capture.return_value {
        fields.insert(
            "return_value".to_owned(),
            serde_json::to_value(return_value).expect("integer type serializes"),
        );
    }
    serde_json::to_value(fields).expect("canonical trace capture serializes")
}

fn validate_identifier(value: &str) -> anyhow::Result<()> {
    let mut chars = value.chars();
    ensure!(
        chars.next().is_some_and(|c| c.is_ascii_alphabetic())
            && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
            && value.len() <= 64,
        "invalid trace point_id"
    );
    Ok(())
}

fn validate_capture_name(value: &str) -> anyhow::Result<()> {
    ensure!(
        !value.is_empty() && value.chars().count() <= 64,
        "invalid trace capture name"
    );
    Ok(())
}

fn validate_module_sha256(value: &str) -> anyhow::Result<()> {
    ensure!(
        value.len() == 64
            && value
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)),
        "invalid trace module_sha256"
    );
    Ok(())
}

fn validate_symbol(value: &str) -> anyhow::Result<()> {
    let mut chars = value.chars();
    ensure!(
        chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_'),
        "trace symbol must be a safe plain symbol"
    );
    Ok(())
}

fn required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Nullable<T> {
        Null(()),
        Value(T),
    }
    Ok(match Nullable::deserialize(deserializer)? {
        Nullable::Value(value) => Some(value),
        Nullable::Null(()) => None,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TraceRunDocument {
    pub schema_version: u32,
    pub kind: String,
    pub backend: Value,
    pub worker_identity: Value,
    pub spec: Value,
    pub logical_plan: Value,
    pub plan_signature: String,
    pub launch: Value,
    #[serde(default)]
    pub bindings: Vec<Value>,
    #[serde(deserialize_with = "required_option")]
    pub completion: Option<TraceCompletion>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TraceCompletion {
    pub stream_closed: bool,
    pub complete: bool,
    pub reason: String,
    pub events_written: u64,
    pub jsonl_bytes: u64,
    pub jsonl_sha256: String,
    pub process: Value,
    #[serde(default)]
    pub limits_hit: Vec<String>,
    #[serde(default)]
    pub capture_errors: Vec<String>,
    #[serde(default)]
    pub open_call_ids: Vec<u64>,
    pub cleanup: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TraceEvent {
    pub schema_version: u32,
    pub event: String,
    pub sequence: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub point_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    pub thread_sequence: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_call_id: Option<u64>,
    #[serde(default)]
    pub arguments: Vec<Value>,
    #[serde(default)]
    pub buffers: Vec<Value>,
    #[serde(deserialize_with = "required_option")]
    pub return_value: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process: Option<Value>,
    #[serde(default)]
    pub capture_errors: Vec<String>,
    pub truncated: bool,
}

/// Parses a complete JSONL event stream and enforces the contiguous sequence fence.
pub fn parse_trace_events<R: std::io::BufRead>(mut reader: R) -> anyhow::Result<Vec<TraceEvent>> {
    let mut events = Vec::new();
    let mut line = Vec::new();
    let mut line_number = 0usize;
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        line_number += 1;
        let terminated = line.last() == Some(&b'\n');
        let payload = if terminated {
            &line[..line.len() - 1]
        } else {
            &line[..]
        };
        let event: TraceEvent = serde_json::from_slice(payload).map_err(|error| {
            if terminated {
                anyhow::anyhow!("invalid trace event JSON on line {line_number}: {error}")
            } else {
                anyhow::anyhow!(
                    "incomplete or invalid final trace event line {line_number}: {error}"
                )
            }
        })?;
        ensure!(
            event.schema_version == 1,
            "unsupported trace event schema_version on line {line_number}"
        );
        ensure!(
            event.sequence == events.len() as u64,
            "non-contiguous or duplicate trace event sequence on line {line_number}: expected {}, got {}",
            events.len(),
            event.sequence
        );
        events.push(event);
    }
    ensure!(!events.is_empty(), "trace event stream is empty");
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Cursor;

    fn golden() -> Value {
        serde_json::from_str(include_str!("../fixtures/trace_plan.json")).unwrap()
    }

    fn golden_spec() -> TraceSpec {
        serde_json::from_value(golden()["spec"].clone()).unwrap()
    }

    fn assert_rejected(value: Value) {
        if let Ok(spec) = serde_json::from_value::<TraceSpec>(value) {
            assert!(spec.validate().is_err(), "specification was accepted");
        }
    }

    #[test]
    fn golden_logical_plan_and_signature() {
        let vector = golden();
        let spec = golden_spec();
        spec.validate().unwrap();
        assert_eq!(
            serde_json::to_string(&spec.logical_plan()).unwrap(),
            vector["canonical_json"].as_str().unwrap()
        );
        assert_eq!(
            spec.plan_signature(),
            vector["expected_signature"].as_str().unwrap()
        );
    }

    #[test]
    fn defaults_and_signature_boundaries() {
        let module = "a".repeat(64);
        let spec: TraceSpec = serde_json::from_value(json!({
            "points": [{
                "kind": "function", "point_id": "f", "phases": ["return"],
                "location": {"kind": "symbol", "module_sha256": module, "symbol": "f"},
                "capture": {"return_value": "pointer"}
            }]
        }))
        .unwrap();
        spec.validate().unwrap();
        assert_eq!(spec.schema_version, 1);
        assert_eq!(spec.abi, TraceAbi::SysvAmd64);
        assert_eq!(spec.limits, TraceLimits::default());
        assert!(spec.points[0].capture().arguments.is_empty());
        assert!(spec.points[0].capture().buffers.is_empty());

        let signature = spec.plan_signature();
        let mut relocated = spec.clone();
        relocated.points[0] = TracePoint::Function {
            point_id: "f".into(),
            location: TraceLocation::ModuleOffset {
                module_sha256: "b".repeat(64),
                offset: 4096,
            },
            phases: vec![FunctionPhase::Return],
            capture: relocated.points[0].capture().clone(),
        };
        relocated.limits.max_events = 4096;
        assert_eq!(relocated.plan_signature(), signature);

        let mut changed = spec.clone();
        if let TracePoint::Function { capture, .. } = &mut changed.points[0] {
            capture.return_value = Some(IntegerType::I64);
        }
        assert_ne!(changed.plan_signature(), signature);
    }

    #[test]
    fn semantic_validation_rejects_invalid_specs() {
        let original = golden()["spec"].clone();
        let mut cases = Vec::new();
        let mut add = |name: &str, mutate: &mut dyn FnMut(&mut Value)| {
            let mut value = original.clone();
            mutate(&mut value);
            cases.push((name.to_owned(), value));
        };
        add("schema", &mut |v| v["schema_version"] = json!(2));
        add("points", &mut |v| v["points"] = json!([]));
        add("identifier", &mut |v| {
            v["points"][0]["point_id"] = json!("0bad")
        });
        add("empty phases", &mut |v| {
            v["points"][0]["phases"] = json!([])
        });
        add("duplicate phases", &mut |v| {
            v["points"][0]["phases"] = json!(["entry", "entry"])
        });
        add("return without phase", &mut |v| {
            v["points"][0]["phases"] = json!(["entry"])
        });
        add("argument index", &mut |v| {
            v["points"][0]["capture"]["arguments"][0]["index"] = json!(6)
        });
        add("buffer address", &mut |v| {
            v["points"][0]["capture"]["buffers"][0]["address_argument"] = json!(6)
        });
        add("buffer length index", &mut |v| {
            v["points"][0]["capture"]["buffers"][0]["length"] =
                json!({"kind": "argument", "index": 6})
        });
        add("constant length", &mut |v| {
            v["points"][0]["capture"]["buffers"][0]["length"] =
                json!({"kind": "constant", "bytes": 1048577})
        });
        add("hash", &mut |v| {
            v["points"][0]["location"]["module_sha256"] = json!("A".repeat(64))
        });
        add("event byte relation", &mut |v| {
            v["limits"]["max_event_bytes"] = json!(65537);
            v["limits"]["max_events_bytes"] = json!(65536);
        });
        for (_name, value) in cases {
            assert_rejected(value);
        }

        for symbol in ["", "main+4", "main()", "x=1", "x;id", "x\nid", "x|id"] {
            let mut value = original.clone();
            value["points"][0]["location"]["symbol"] = json!(symbol);
            assert_rejected(value);
        }
        for (field, value) in [
            ("max_events", 0),
            ("max_point_hits", 0),
            ("max_buffer_bytes", 0),
            ("max_event_bytes", 1023),
            ("max_events_bytes", 4095),
            ("stop_wait_ms", 99),
        ] {
            let mut spec = original.clone();
            spec["limits"][field] = json!(value);
            assert_rejected(spec);
        }
    }

    #[test]
    fn duplicate_names_indices_ids_and_unknown_or_overflow_fields_fail() {
        let mut duplicate_ids = golden()["spec"].clone();
        let point = duplicate_ids["points"][0].clone();
        duplicate_ids["points"].as_array_mut().unwrap().push(point);
        assert_rejected(duplicate_ids);

        let mut duplicate_indices = golden()["spec"].clone();
        let argument = duplicate_indices["points"][0]["capture"]["arguments"][0].clone();
        duplicate_indices["points"][0]["capture"]["arguments"]
            .as_array_mut()
            .unwrap()
            .push(argument);
        assert_rejected(duplicate_indices);

        let mut duplicate_names = golden()["spec"].clone();
        duplicate_names["points"][0]["capture"]["buffers"][0]["name"] =
            duplicate_names["points"][0]["capture"]["arguments"][0]["name"].clone();
        assert_rejected(duplicate_names);

        let mut unknown = golden()["spec"].clone();
        unknown["unexpected"] = json!(true);
        assert!(serde_json::from_value::<TraceSpec>(unknown).is_err());
        let mut overflow = golden()["spec"].clone();
        overflow["points"][0]["location"] = json!({
            "kind": "module_offset", "module_sha256": "a".repeat(64),
            "offset": serde_json::from_str::<Value>("18446744073709551616").unwrap()
        });
        assert!(serde_json::from_value::<TraceSpec>(overflow).is_err());
    }

    fn event(sequence: u64) -> String {
        serde_json::to_string(&json!({
            "schema_version": 1, "event": "call", "sequence": sequence,
            "thread_sequence": 0, "return_value": null, "truncated": false
        }))
        .unwrap()
    }

    #[test]
    fn event_parser_enforces_jsonl_and_sequence_fences() {
        let events =
            parse_trace_events(Cursor::new(format!("{}\n{}", event(0), event(1)))).unwrap();
        assert_eq!(events.len(), 2);
        assert!(events[0].return_value.is_none());
        let mut exit: Value = serde_json::from_str(&event(0)).unwrap();
        exit["event"] = json!("exit");
        exit["process"] = json!({"reason": "exited", "exit_code": 0});
        let exit = parse_trace_events(Cursor::new(exit.to_string())).unwrap();
        assert_eq!(exit[0].process.as_ref().unwrap()["exit_code"], 0);
        assert!(parse_trace_events(Cursor::new("")).is_err());
        assert!(parse_trace_events(Cursor::new("{")).is_err());
        assert!(parse_trace_events(Cursor::new(format!("{}\n{}", event(0), event(0)))).is_err());
        assert!(parse_trace_events(Cursor::new(format!("{}\n{}", event(0), event(2)))).is_err());
        assert!(parse_trace_events(Cursor::new("{\"event\":\"call\"}\n")).is_err());
        let missing_return = json!({
            "schema_version": 1, "event": "call", "sequence": 0,
            "thread_sequence": 0, "truncated": false
        });
        assert!(parse_trace_events(Cursor::new(missing_return.to_string())).is_err());
        assert!(parse_trace_events(Cursor::new(format!("{}\n{{", event(0)))).is_err());
    }
}
