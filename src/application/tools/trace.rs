//! Offline comparison of imported dynamic-trace evidence.
use super::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::BufReader,
};
use wukong_worker_protocol::{
    TraceEvent, TraceRunDocument, TraceSpec, params::TraceStartArgs, parse_trace_events,
};

const COMPARISON_DOMAIN: &[u8] = b"wukong.trace.comparison.v1\0";
const MAX_REASONS: usize = 16;

#[derive(Clone, Copy)]
enum Side {
    Left,
    Right,
}
impl Side {
    fn label(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
        }
    }
}

struct ResolvedSide {
    side: Side,
    job: JobRecord,
    run: Evidence,
    events: Evidence,
}

struct PreparedSide {
    document: Option<TraceRunDocument>,
    spec: Option<TraceSpec>,
    reasons: Vec<String>,
    incompatible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct EventKey {
    point_id: Option<String>,
    phase: Option<String>,
    thread_sequence: u64,
    call_id: Option<u64>,
}
impl EventKey {
    fn of(event: &TraceEvent) -> Self {
        Self {
            point_id: event.point_id.clone(),
            phase: event.phase.clone(),
            thread_sequence: event.thread_sequence,
            call_id: event.call_id,
        }
    }
    fn json(&self) -> Value {
        json!({"point_id":self.point_id,"phase":self.phase,"thread_sequence":self.thread_sequence,"call_id":self.call_id})
    }
}

pub(super) fn compare(
    tools: &evidence::EvidenceService,
    agent: &AgentMetadata,
    call: &str,
    input: args::TraceCompareArgs,
) -> Result<Value> {
    validate_options(&input.comparison)?;
    let options = serde_json::to_value(&input.comparison)?;
    let left = resolve_side(&tools.store, agent, Side::Left, &input.left)?;
    let right = resolve_side(&tools.store, agent, Side::Right, &input.right)?;
    ensure!(
        left.job.id != right.job.id,
        "left and right traces must come from different jobs"
    );

    let left_prepared = prepare_side(&tools.store, &left)?;
    let right_prepared = prepare_side(&tools.store, &right)?;
    if !left_prepared.reasons.is_empty() || !right_prepared.reasons.is_empty() {
        let reasons = [
            left_prepared.reasons.clone(),
            right_prepared.reasons.clone(),
        ]
        .concat();
        let verdict = if left_prepared.incompatible || right_prepared.incompatible {
            "incomparable"
        } else {
            "inconclusive"
        };
        let result = base_result(
            &left,
            &right,
            left_prepared.document.as_ref(),
            right_prepared.document.as_ref(),
            &options,
            verdict,
            reasons,
            0,
            0,
        );
        return publish(tools, agent, call, result);
    }
    let left_document = left_prepared.document.as_ref().expect("prepared left");
    let right_document = right_prepared.document.as_ref().expect("prepared right");
    let left_spec = left_prepared.spec.as_ref().expect("validated left spec");
    let right_spec = right_prepared.spec.as_ref().expect("validated right spec");
    let incompatible = compatibility_reasons(left_document, right_document, left_spec, right_spec);
    if !incompatible.is_empty() {
        let result = base_result(
            &left,
            &right,
            Some(left_document),
            Some(right_document),
            &options,
            "incomparable",
            incompatible,
            0,
            0,
        );
        return publish(tools, agent, call, result);
    }

    validate_capture_selection(left_spec, &input.comparison)?;
    let (left_events, mut left_reasons) = load_events(&tools.store, &left, left_document)?;
    let (right_events, mut right_reasons) = load_events(&tools.store, &right, right_document)?;
    left_reasons.extend(completeness_reasons(&left, left_document, &left_events));
    right_reasons.extend(completeness_reasons(&right, right_document, &right_events));
    left_reasons.extend(validate_events(&left_events, left_spec));
    right_reasons.extend(validate_events(&right_events, right_spec));
    truncate_reasons(&mut left_reasons);
    truncate_reasons(&mut right_reasons);
    if !left_reasons.is_empty() || !right_reasons.is_empty() {
        let mut result = base_result(
            &left,
            &right,
            Some(left_document),
            Some(right_document),
            &options,
            "inconclusive",
            [left_reasons, right_reasons].concat(),
            left_events.len(),
            right_events.len(),
        );
        result["counts"]["left_events"] = json!(left_events.len());
        result["counts"]["right_events"] = json!(right_events.len());
        return publish(tools, agent, call, result);
    }

    let left_map = event_map(&left_events)?;
    let right_map = event_map(&right_events)?;
    let comparison = compare_events(
        &left_events,
        &right_events,
        &left_map,
        &right_map,
        left_spec,
        &input.comparison,
    )?;
    let verdict = if comparison.total_differences == 0 {
        "match"
    } else {
        "mismatch"
    };
    let mut result = base_result(
        &left,
        &right,
        Some(left_document),
        Some(right_document),
        &options,
        verdict,
        Vec::new(),
        left_events.len(),
        right_events.len(),
    );
    result["counts"]["compared_events"] = json!(comparison.compared_events);
    result["counts"]["compared_captures"] = json!(comparison.compared_captures);
    result["counts"]["differences"] = json!(comparison.total_differences);
    result["differences"] = Value::Array(comparison.differences);
    result["differences_truncated"] =
        json!(comparison.total_differences > input.comparison.max_differences as usize);
    publish(tools, agent, call, result)
}

fn validate_options(options: &args::TraceComparisonArgs) -> Result<()> {
    ensure!(
        (1..=64).contains(&options.max_differences),
        "max_differences must be in 1..64"
    );
    for id in options
        .capture_ids
        .iter()
        .flatten()
        .chain(options.ignore.iter())
    {
        validate_capture_id(id)?;
    }
    if let Some(selected) = &options.capture_ids {
        ensure!(!selected.is_empty(), "capture_ids must not be empty");
        ensure!(
            selected.iter().collect::<BTreeSet<_>>().len() == selected.len(),
            "duplicate capture_ids entry"
        );
    }
    Ok(())
}

fn validate_capture_id(id: &str) -> Result<()> {
    let (point, capture) = id
        .split_once(':')
        .context("capture ID must use point_id:capture_name")?;
    ensure!(
        !point.is_empty() && !capture.is_empty() && id.chars().count() <= 129,
        "invalid capture ID"
    );
    Ok(())
}

fn validate_capture_selection(spec: &TraceSpec, options: &args::TraceComparisonArgs) -> Result<()> {
    let Some(selected) = &options.capture_ids else {
        return Ok(());
    };
    let declared = declared_captures(spec);
    for id in selected {
        let (point, capture) = id.split_once(':').expect("validated capture ID");
        ensure!(
            declared
                .get(point)
                .is_some_and(|captures| captures.contains(capture)),
            "capture ID is not declared by the trace plan: {id}"
        );
    }
    Ok(())
}

fn resolve_side(
    store: &Store,
    agent: &AgentMetadata,
    side: Side,
    reference: &args::TraceRunEvidenceArgs,
) -> Result<ResolvedSide> {
    let run: Evidence = require_in_run(store, &reference.run_evidence_id, &agent.run_id)?;
    let events: Evidence = require_in_run(store, &reference.events_evidence_id, &agent.run_id)?;
    ensure!(run.id != events.id, "run and events evidence must differ");
    let mut candidates = Vec::new();
    for job in store.records::<JobRecord>(&agent.run_id)? {
        if job.tool_name != "trace_run" || job.agent_id != agent.id {
            continue;
        }
        let Some(artifacts) = job
            .imported_artifacts
            .as_ref()
            .and_then(|imported| imported["artifacts"].as_array())
        else {
            continue;
        };
        let references = artifacts.iter().any(|artifact| {
            matches!(artifact["evidence_id"].as_str(), Some(id) if id == run.id || id == events.id)
        });
        if references {
            candidates.push(job);
        }
    }
    ensure!(
        candidates.len() == 1,
        "{} evidence must resolve uniquely to one caller-owned trace_run job",
        side.label()
    );
    let job = candidates.remove(0);
    validate_imported_artifact(&job, &run, "/trace/run.json")?;
    validate_imported_artifact(&job, &events, "/trace/events.jsonl")?;
    Ok(ResolvedSide {
        side,
        job,
        run,
        events,
    })
}

fn validate_imported_artifact(job: &JobRecord, evidence: &Evidence, suffix: &str) -> Result<()> {
    let artifacts = job
        .imported_artifacts
        .as_ref()
        .and_then(|imported| imported["artifacts"].as_array())
        .context("trace job artifact manifest missing")?;
    let matching = artifacts
        .iter()
        .filter(|artifact| artifact["evidence_id"] == evidence.id)
        .collect::<Vec<_>>();
    ensure!(
        matching.len() == 1,
        "trace evidence must occur exactly once in the job artifact manifest"
    );
    let artifact = matching[0]["artifact"].clone();
    let path = artifact["path"]
        .as_str()
        .context("trace artifact path missing")?;
    ensure!(
        path.ends_with(suffix),
        "trace artifact has the wrong path for {suffix}"
    );
    ensure!(
        artifact["sha256"] == evidence.sha256 && artifact["size"] == evidence.size,
        "trace artifact path/hash/size does not match its evidence"
    );
    let suffix_entries = artifacts
        .iter()
        .filter(|candidate| {
            candidate["artifact"]["path"]
                .as_str()
                .is_some_and(|path| path.ends_with(suffix))
        })
        .count();
    ensure!(
        suffix_entries == 1,
        "trace job must contain exactly one {suffix} artifact"
    );
    Ok(())
}

fn prepare_side(store: &Store, side: &ResolvedSide) -> Result<PreparedSide> {
    let bytes = store.read_evidence(&side.job.run_id, &side.run.id)?;
    let mut raw: Value = match serde_json::from_slice(&bytes) {
        Ok(raw) => raw,
        Err(error) => {
            return Ok(PreparedSide {
                document: None,
                spec: None,
                reasons: vec![format!(
                    "{} run.json is not valid JSON: {}",
                    side.side.label(),
                    bounded_error(&error)
                )],
                incompatible: false,
            });
        }
    };
    // A missing completion is an incomplete run, not an incompatible schema. The shared
    // protocol intentionally rejects a missing required_option during typed deserialization,
    // so normalize only that absence (and a failed stream's null digest) before parsing.
    if let Some(object) = raw.as_object_mut() {
        object.entry("completion").or_insert(Value::Null);
        if let Some(completion) = object.get_mut("completion").and_then(Value::as_object_mut)
            && completion.get("jsonl_sha256") == Some(&Value::Null)
        {
            completion.insert("jsonl_sha256".into(), Value::String(String::new()));
        }
    }
    let document: TraceRunDocument = match serde_json::from_value(raw) {
        Ok(document) => document,
        Err(error) => {
            return Ok(PreparedSide {
                document: None,
                spec: None,
                reasons: vec![format!(
                    "{} run.json is not a supported TraceRunDocument: {}",
                    side.side.label(),
                    bounded_error(&error)
                )],
                incompatible: false,
            });
        }
    };
    let mut reasons = Vec::new();
    if document.schema_version != 1 || document.kind != "wukong.trace.run" {
        reasons.push(format!(
            "{} run.json schema/kind is unsupported",
            side.side.label()
        ));
        return Ok(PreparedSide {
            document: Some(document),
            spec: None,
            reasons,
            incompatible: true,
        });
    }
    let parameters: TraceStartArgs = serde_json::from_value(side.job.parameters.clone())
        .context("trace_run job parameters are invalid")?;
    ensure!(
        serde_json::to_value(&parameters.spec)? == document.spec,
        "{} run.json spec differs from trace_run job parameters",
        side.side.label()
    );
    let spec: TraceSpec = match serde_json::from_value(document.spec.clone()) {
        Ok(spec) => spec,
        Err(error) => {
            reasons.push(format!(
                "{} trace spec is unsupported: {}",
                side.side.label(),
                bounded_error(&error)
            ));
            return Ok(PreparedSide {
                document: Some(document),
                spec: None,
                reasons,
                incompatible: true,
            });
        }
    };
    if let Err(error) = spec.validate() {
        reasons.push(format!(
            "{} trace spec is invalid: {}",
            side.side.label(),
            bounded_error(&error)
        ));
        return Ok(PreparedSide {
            document: Some(document),
            spec: None,
            reasons,
            incompatible: true,
        });
    }
    if reasons.is_empty() {
        validate_document_binding(store, side, &document, &spec, &parameters)?;
    }
    Ok(PreparedSide {
        document: Some(document),
        spec: Some(spec),
        reasons,
        incompatible: false,
    })
}

fn validate_document_binding(
    store: &Store,
    side: &ResolvedSide,
    document: &TraceRunDocument,
    spec: &TraceSpec,
    parameters: &TraceStartArgs,
) -> Result<()> {
    ensure!(
        document.logical_plan == spec.logical_plan()
            && document.plan_signature == spec.plan_signature(),
        "{} run.json logical plan/signature does not match its spec",
        side.side.label()
    );
    let launch = &document.launch;
    let manifest = &launch["manifest"];
    let launch_file = worker_file(store, side, &parameters.launch_spec)?;
    let manifest_evidence_id = launch_file
        .evidence_id
        .as_deref()
        .context("LaunchSpec has no same-run content evidence")?;
    let manifest_bytes = store.read_evidence(&side.job.run_id, manifest_evidence_id)?;
    let manifest_hash = hex::encode(Sha256::digest(&manifest_bytes));
    ensure!(
        manifest_bytes.len() <= 65536
            && manifest_hash == launch_file.sha256
            && launch["manifest_sha256"] == manifest_hash,
        "{} run.json LaunchSpec identity mismatch",
        side.side.label()
    );
    let recorded_manifest: Value = serde_json::from_slice(&manifest_bytes)?;
    ensure!(
        &recorded_manifest == manifest,
        "{} run.json LaunchSpec content differs from its registered evidence",
        side.side.label()
    );
    let target = manifest["target"]
        .as_str()
        .context("LaunchSpec target member missing")?;
    ensure!(
        launch["target_member"] == target
            && manifest["files"][target] == side.job.target_sha256
            && launch["source_target_sha256"] == side.job.target_sha256,
        "{} run.json source target does not match its job",
        side.side.label()
    );
    ensure!(
        launch["executed_target_sha256"]
            .as_str()
            .is_some_and(valid_hash),
        "{} run.json executed target identity is invalid",
        side.side.label()
    );
    for point in &spec.points {
        let module = match point.location() {
            wukong_worker_protocol::TraceLocation::Symbol { module_sha256, .. }
            | wukong_worker_protocol::TraceLocation::ModuleOffset { module_sha256, .. } => {
                module_sha256
            }
        };
        ensure!(
            module == &side.job.target_sha256,
            "{} trace point module differs from the source target",
            side.side.label()
        );
    }

    let input_file = worker_file(store, side, &parameters.input)?;
    let input_evidence_id = input_file
        .evidence_id
        .as_deref()
        .context("trace input has no same-run content evidence")?;
    let input_evidence: Evidence = require_in_run(store, input_evidence_id, &side.job.run_id)?;
    ensure!(
        input_evidence.sha256 == input_file.sha256 && input_evidence.size == input_file.size,
        "{} trace input evidence identity mismatch",
        side.side.label()
    );
    store.verified_evidence_path(&side.job.run_id, input_evidence_id)?;
    ensure!(
        launch["input"]["sha256"] == input_file.sha256
            && launch["input"]["size"] == input_file.size
            && launch["input"]["mode"] == manifest["input_mode"],
        "{} run.json input identity/mode mismatch",
        side.side.label()
    );
    ensure!(
        launch["environment_signature"]
            .as_str()
            .is_some_and(valid_hash),
        "{} run.json environment signature is invalid",
        side.side.label()
    );
    Ok(())
}

fn worker_file(store: &Store, side: &ResolvedSide, path: &str) -> Result<WorkerFileRecord> {
    let files = store
        .records::<WorkerFileRecord>(&side.job.run_id)?
        .into_iter()
        .filter(|file| {
            file.worker_id == side.job.worker_id
                && file.run_id == side.job.run_id
                && file.path == path
        })
        .collect::<Vec<_>>();
    ensure!(
        files.len() == 1,
        "{} trace parameter path is not uniquely registered in its run/worker: {path}",
        side.side.label()
    );
    Ok(files.into_iter().next().expect("one worker file"))
}

fn compatibility_reasons(
    left: &TraceRunDocument,
    right: &TraceRunDocument,
    left_spec: &TraceSpec,
    right_spec: &TraceSpec,
) -> Vec<String> {
    let mut reasons = Vec::new();
    let mut check = |different: bool, reason: &str| {
        if different {
            reasons.push(reason.to_owned());
        }
    };
    check(
        left.schema_version != right.schema_version || left.kind != right.kind,
        "trace run schema/kind differs",
    );
    check(
        left_spec.schema_version != right_spec.schema_version || left_spec.abi != right_spec.abi,
        "trace spec schema/ABI differs",
    );
    check(
        left.plan_signature != right.plan_signature || left.logical_plan != right.logical_plan,
        "trace logical plan signature differs",
    );
    check(
        left.worker_identity != right.worker_identity,
        "worker identity differs",
    );
    check(left.backend != right.backend, "trace backend differs");
    check(
        left.launch["input"]["sha256"] != right.launch["input"]["sha256"]
            || left.launch["input"]["size"] != right.launch["input"]["size"]
            || left.launch["input"]["mode"] != right.launch["input"]["mode"],
        "trace input SHA256/size/mode differs",
    );
    check(
        left.launch["environment_signature"] != right.launch["environment_signature"],
        "trace environment signature differs",
    );
    check(
        non_target_launch(left) != non_target_launch(right),
        "non-target launch conditions differ",
    );
    reasons
}

fn non_target_launch(document: &TraceRunDocument) -> Value {
    let mut manifest = document.launch["manifest"].clone();
    if let Some(object) = manifest.as_object_mut() {
        object.remove("base");
        if let Some(target) = object
            .get("target")
            .and_then(Value::as_str)
            .map(str::to_owned)
            && let Some(files) = object.get_mut("files").and_then(Value::as_object_mut)
        {
            files.remove(&target);
        }
    }
    manifest
}

fn load_events(
    store: &Store,
    side: &ResolvedSide,
    document: &TraceRunDocument,
) -> Result<(Vec<TraceEvent>, Vec<String>)> {
    let path = store.verified_evidence_path(&side.job.run_id, &side.events.id)?;
    let file = File::open(path)?;
    let events = match parse_trace_events(BufReader::new(file)) {
        Ok(events) => events,
        Err(error) => {
            return Ok((
                Vec::new(),
                vec![format!(
                    "{} events.jsonl is incomplete or invalid: {}",
                    side.side.label(),
                    bounded_error(&error)
                )],
            ));
        }
    };
    let mut reasons = Vec::new();
    if events.is_empty() || events.iter().all(|event| event.point_id.is_none()) {
        reasons.push(format!(
            "{} trace contains zero capture events; an empty observation can never match",
            side.side.label()
        ));
    }
    if let Some(completion) = &document.completion {
        if completion.events_written != events.len() as u64 {
            reasons.push(format!(
                "{} completion event count differs from events.jsonl",
                side.side.label()
            ));
        }
        if completion.jsonl_bytes != side.events.size
            || completion.jsonl_sha256 != side.events.sha256
        {
            reasons.push(format!(
                "{} completion JSONL hash/size differs from imported evidence",
                side.side.label()
            ));
        }
    }
    Ok((events, reasons))
}

fn completeness_reasons(
    side: &ResolvedSide,
    document: &TraceRunDocument,
    events: &[TraceEvent],
) -> Vec<String> {
    let mut reasons = Vec::new();
    let label = side.side.label();
    if side.job.status.state != JobState::Completed {
        reasons.push(format!("{label} trace job is not completed"));
    }
    if !side.job.status.settled() {
        reasons.push(format!("{label} trace job cleanup is not settled"));
    }
    if !side.job.evidence_ready() {
        reasons.push(format!("{label} trace evidence is not ready"));
    }
    let Some(completion) = &document.completion else {
        reasons.push(format!("{label} trace completion is missing"));
        return reasons;
    };
    if !completion.stream_closed {
        reasons.push(format!("{label} trace event stream is not closed"));
    }
    if !completion.complete {
        reasons.push(format!("{label} trace completion is incomplete"));
    }
    if !completion.limits_hit.is_empty() {
        reasons.push(format!("{label} trace hit capture limits"));
    }
    if !completion.capture_errors.is_empty() {
        reasons.push(format!("{label} trace completion contains capture errors"));
    }
    if !completion.open_call_ids.is_empty() {
        reasons.push(format!("{label} trace has open call IDs"));
    }
    for event in events {
        if !event.capture_errors.is_empty() {
            reasons.push(format!(
                "{label} event {} contains capture errors",
                event.sequence
            ));
        }
        if event.truncated {
            reasons.push(format!("{label} event {} is truncated", event.sequence));
        }
    }
    reasons
}

fn validate_events(events: &[TraceEvent], spec: &TraceSpec) -> Vec<String> {
    let declared = declared_captures(spec);
    let mut reasons = Vec::new();
    let mut keys = BTreeSet::new();
    for event in events {
        if !keys.insert(EventKey::of(event)) {
            reasons.push(format!(
                "duplicate event key at sequence {}",
                event.sequence
            ));
        }
        let Some(point) = event.point_id.as_deref() else {
            continue;
        };
        let Some(captures) = declared.get(point) else {
            reasons.push(format!(
                "event {} references an undeclared point",
                event.sequence
            ));
            continue;
        };
        let mut names = BTreeSet::new();
        for argument in &event.arguments {
            let Some(name) = argument["name"].as_str() else {
                reasons.push(format!(
                    "capture name is missing in event {}",
                    event.sequence
                ));
                continue;
            };
            if !names.insert(name.to_owned()) {
                reasons.push(format!(
                    "duplicate capture name in event {}",
                    event.sequence
                ));
            }
            if captures.contains(name)
                && let Err(error) = normalized_integer(argument)
            {
                reasons.push(format!(
                    "invalid typed integer in event {}: {}",
                    event.sequence,
                    bounded_error(&error)
                ));
            }
        }
        if let Some(return_value) = &event.return_value
            && captures.contains("return_value")
            && let Err(error) = normalized_integer(return_value)
        {
            reasons.push(format!(
                "invalid typed return value in event {}: {}",
                event.sequence,
                bounded_error(&error)
            ));
        }
        for buffer in &event.buffers {
            if let Some(name) = buffer["name"].as_str() {
                if !names.insert(name.to_owned()) {
                    reasons.push(format!(
                        "duplicate capture name in event {}",
                        event.sequence
                    ));
                }
            } else {
                reasons.push(format!(
                    "buffer name is missing in event {}",
                    event.sequence
                ));
            }
            if buffer["status"] != "ok" || buffer["truncated"] == true || !buffer["error"].is_null()
            {
                reasons.push(format!("incomplete buffer in event {}", event.sequence));
            }
            if !buffer["data_base64"].is_string() {
                reasons.push(format!(
                    "buffer data is missing in event {}",
                    event.sequence
                ));
            }
            match BASE64.decode(buffer["data_base64"].as_str().unwrap_or_default()) {
                Ok(bytes) => {
                    if buffer["bytes_read"].as_u64() != Some(bytes.len() as u64) {
                        reasons.push(format!(
                            "buffer byte count mismatch in event {}",
                            event.sequence
                        ));
                    }
                }
                Err(_) => {
                    reasons.push(format!("invalid buffer base64 in event {}", event.sequence))
                }
            }
            if buffer["length_value"].as_u64().is_none()
                || buffer["requested_bytes"].as_u64().is_none()
                || buffer["bytes_read"].as_u64().is_none()
            {
                reasons.push(format!(
                    "buffer length is missing in event {}",
                    event.sequence
                ));
            }
        }
    }
    truncate_reasons(&mut reasons);
    reasons
}

fn declared_captures(spec: &TraceSpec) -> BTreeMap<String, BTreeSet<String>> {
    spec.points
        .iter()
        .map(|point| {
            let capture = point.capture();
            let mut names = capture
                .arguments
                .iter()
                .map(|argument| argument.name.clone())
                .chain(capture.buffers.iter().map(|buffer| buffer.name.clone()))
                .collect::<BTreeSet<_>>();
            if capture.return_value.is_some()
                || matches!(point, wukong_worker_protocol::TracePoint::Function { .. })
            {
                names.insert("return_value".into());
            }
            (point.point_id().to_owned(), names)
        })
        .collect()
}

fn event_map(events: &[TraceEvent]) -> Result<BTreeMap<EventKey, &TraceEvent>> {
    let mut mapped = BTreeMap::new();
    for event in events {
        ensure!(
            mapped.insert(EventKey::of(event), event).is_none(),
            "duplicate event key after completeness validation"
        );
    }
    Ok(mapped)
}

struct EventComparison {
    differences: Vec<Value>,
    total_differences: usize,
    compared_events: usize,
    compared_captures: usize,
}

fn compare_events(
    left_events: &[TraceEvent],
    right_events: &[TraceEvent],
    left: &BTreeMap<EventKey, &TraceEvent>,
    right: &BTreeMap<EventKey, &TraceEvent>,
    spec: &TraceSpec,
    options: &args::TraceComparisonArgs,
) -> Result<EventComparison> {
    let mut result = EventComparison {
        differences: Vec::new(),
        total_differences: 0,
        compared_events: 0,
        compared_captures: 0,
    };
    let mut record = |difference: Value| {
        result.total_differences += 1;
        if result.differences.len() < options.max_differences as usize {
            result.differences.push(difference);
        }
    };
    for index in 0..left_events.len().max(right_events.len()) {
        let left_key = left_events.get(index).map(EventKey::of);
        let right_key = right_events.get(index).map(EventKey::of);
        if left_key != right_key {
            record(
                json!({"kind":"event_order","left_sequence":left_events.get(index).map(|e|e.sequence),
                "right_sequence":right_events.get(index).map(|e|e.sequence),
                "left_key":left_key.map(|k|k.json()),"right_key":right_key.map(|k|k.json())}),
            );
        }
    }
    let keys = left
        .keys()
        .chain(right.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let declared = declared_captures(spec);
    for key in keys {
        let left_event = left.get(&key).copied();
        let right_event = right.get(&key).copied();
        match (left_event, right_event) {
            (Some(left), Some(right)) => {
                result.compared_events += 1;
                if left.event != right.event {
                    record(
                        json!({"kind":"event_kind","left_sequence":left.sequence,"right_sequence":right.sequence,
                        "left":left.event,"right":right.event}),
                    );
                }
                let left_captures = selected_captures(left, &declared, options)?;
                let right_captures = selected_captures(right, &declared, options)?;
                let capture_names = left_captures
                    .keys()
                    .chain(right_captures.keys())
                    .cloned()
                    .collect::<BTreeSet<_>>();
                for name in capture_names {
                    result.compared_captures += 1;
                    let capture_id = format!("{}:{name}", key.point_id.as_deref().unwrap_or(""));
                    match (left_captures.get(&name), right_captures.get(&name)) {
                        (Some(left_value), Some(right_value)) => {
                            if is_buffer(left_value) || is_buffer(right_value) {
                                if let Some(difference) = buffer_difference(
                                    &capture_id,
                                    left,
                                    right,
                                    left_value,
                                    right_value,
                                )? {
                                    record(difference);
                                }
                            } else {
                                let left_integer = normalized_integer(left_value)?;
                                let right_integer = normalized_integer(right_value)?;
                                if left_integer != right_integer {
                                    record(json!({"kind":"integer","capture_id":capture_id,
                                        "left_sequence":left.sequence,"right_sequence":right.sequence,
                                        "left":left_integer,"right":right_integer}));
                                }
                            }
                        }
                        _ => record(json!({"kind":"missing_capture","capture_id":capture_id,
                            "left_sequence":left.sequence,"right_sequence":right.sequence,
                            "left_present":left_captures.contains_key(&name),
                            "right_present":right_captures.contains_key(&name)})),
                    }
                }
                let left_process = left.process.as_ref().map(semantic_process);
                let right_process = right.process.as_ref().map(semantic_process);
                if left_process != right_process {
                    record(
                        json!({"kind":"process","left_sequence":left.sequence,"right_sequence":right.sequence,
                        "left":left_process,"right":right_process}),
                    );
                }
            }
            _ => record(json!({"kind":"missing_event","key":key.json(),
                "left_present":left_event.is_some(),"right_present":right_event.is_some()})),
        }
    }
    Ok(result)
}

fn selected_captures(
    event: &TraceEvent,
    declared: &BTreeMap<String, BTreeSet<String>>,
    options: &args::TraceComparisonArgs,
) -> Result<BTreeMap<String, Value>> {
    let mut captures = BTreeMap::new();
    let Some(point) = event.point_id.as_deref() else {
        return Ok(captures);
    };
    let declared = declared.get(point).cloned().unwrap_or_default();
    let wanted = |name: &str| {
        declared.contains(name)
            && !options.ignore.contains(&format!("{point}:{name}"))
            && options
                .capture_ids
                .as_ref()
                .is_none_or(|selected| selected.contains(&format!("{point}:{name}")))
    };
    for value in event.arguments.iter().chain(event.buffers.iter()) {
        let name = value["name"]
            .as_str()
            .context("capture name missing after validation")?;
        if wanted(name) {
            ensure!(
                captures.insert(name.to_owned(), value.clone()).is_none(),
                "duplicate selected capture"
            );
        }
    }
    if wanted("return_value")
        && let Some(value) = &event.return_value
    {
        captures.insert("return_value".into(), value.clone());
    }
    Ok(captures)
}

fn is_buffer(value: &Value) -> bool {
    value.get("data_base64").is_some() || value.get("address_argument").is_some()
}

fn normalized_integer(value: &Value) -> Result<Value> {
    if value["type"].is_null() {
        let raw = value["raw"]
            .as_str()
            .context("untyped integer has no raw register value")?;
        let digits = raw.strip_prefix("0x").unwrap_or(raw);
        let value = u64::from_str_radix(digits, 16).context("invalid raw register value")?;
        return Ok(json!({"type":"raw_u64","value":format!("0x{value:016x}")}));
    }
    let kind = value["type"]
        .as_str()
        .context("typed integer has no type")?;
    let text = match &value["value"] {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        _ => bail!("typed integer value is not a string or number"),
    };
    let normalized = match kind {
        "pointer" => {
            let value = if let Some(digits) = text.strip_prefix("0x") {
                u64::from_str_radix(digits, 16).context("invalid pointer value")?
            } else {
                text.parse::<u64>().context("invalid pointer value")?
            };
            format!("0x{value:x}")
        }
        "u8" | "u16" | "u32" | "u64" => {
            let value = text.parse::<u128>().context("invalid unsigned integer")?;
            let bits = kind[1..].parse::<u32>().expect("unsigned integer width");
            ensure!(
                value < (1_u128 << bits),
                "unsigned integer exceeds its width"
            );
            value.to_string()
        }
        "i8" | "i16" | "i32" | "i64" => {
            let value = text.parse::<i128>().context("invalid signed integer")?;
            let bits = kind[1..].parse::<u32>().expect("signed integer width");
            let minimum = -(1_i128 << (bits - 1));
            let maximum = (1_i128 << (bits - 1)) - 1;
            ensure!(
                (minimum..=maximum).contains(&value),
                "signed integer exceeds its width"
            );
            value.to_string()
        }
        _ => bail!("unsupported typed integer kind"),
    };
    Ok(json!({"type":kind,"value":normalized}))
}

fn buffer_difference(
    capture_id: &str,
    left_event: &TraceEvent,
    right_event: &TraceEvent,
    left: &Value,
    right: &Value,
) -> Result<Option<Value>> {
    let left_bytes = BASE64.decode(
        left["data_base64"]
            .as_str()
            .context("left buffer data missing")?,
    )?;
    let right_bytes = BASE64.decode(
        right["data_base64"]
            .as_str()
            .context("right buffer data missing")?,
    )?;
    let common = left_bytes.len().min(right_bytes.len());
    let mut byte_differences = (0..common)
        .filter(|index| left_bytes[*index] != right_bytes[*index])
        .map(|index| (index, Some(left_bytes[index]), Some(right_bytes[index])))
        .chain(
            (common..left_bytes.len().max(right_bytes.len())).map(|index| {
                (
                    index,
                    left_bytes.get(index).copied(),
                    right_bytes.get(index).copied(),
                )
            }),
        )
        .collect::<Vec<_>>();
    let different_byte_count = byte_differences.len();
    byte_differences.truncate(8);
    let first_difference_offset = byte_differences.first().map(|entry| entry.0);
    let samples = byte_differences
        .into_iter()
        .map(|(offset, left, right)| json!({"offset":offset,"left":left,"right":right}))
        .collect::<Vec<_>>();
    let left_metadata = buffer_metadata(left);
    let right_metadata = buffer_metadata(right);
    if different_byte_count == 0 && left_metadata == right_metadata {
        return Ok(None);
    }
    Ok(Some(json!({
        "kind":"buffer",
        "capture_id":capture_id,
        "left_sequence":left_event.sequence,
        "right_sequence":right_event.sequence,
        "first_difference_offset":first_difference_offset,
        "different_byte_count":different_byte_count,
        "left_length":left_bytes.len(),
        "right_length":right_bytes.len(),
        "left_sha256":hex::encode(Sha256::digest(&left_bytes)),
        "right_sha256":hex::encode(Sha256::digest(&right_bytes)),
        "byte_differences":samples,
        "left_metadata":left_metadata,
        "right_metadata":right_metadata,
    })))
}

fn buffer_metadata(buffer: &Value) -> Value {
    let address = buffer["address"].as_object().map(|address| {
        address
            .iter()
            .filter(|(key, _)| {
                !matches!(
                    key.as_str(),
                    "raw" | "runtime_address" | "runtime_va" | "load_bias"
                )
            })
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<serde_json::Map<_, _>>()
    });
    json!({
        "length_value":buffer["length_value"],
        "requested_bytes":buffer["requested_bytes"],
        "bytes_read":buffer["bytes_read"],
        "status":buffer["status"],
        "truncated":buffer["truncated"],
        "address":address,
    })
}

fn semantic_process(process: &Value) -> Value {
    const SEMANTIC: &[&str] = &[
        "reason",
        "exit-code",
        "exit_code",
        "exit-status",
        "exit_status",
        "signal",
        "signal-name",
        "signal_name",
        "signal-meaning",
        "signal_meaning",
        "errno",
        "errno-name",
        "status",
    ];
    let fields = process
        .as_object()
        .map(|process| {
            process
                .iter()
                .filter(|(key, _)| SEMANTIC.contains(&key.as_str()))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<serde_json::Map<_, _>>()
        })
        .unwrap_or_default();
    Value::Object(fields)
}

#[expect(
    clippy::too_many_arguments,
    reason = "one result binds both evidence sides, their parsed identities, options, verdict and event counts"
)]
fn base_result(
    left: &ResolvedSide,
    right: &ResolvedSide,
    left_document: Option<&TraceRunDocument>,
    right_document: Option<&TraceRunDocument>,
    options: &Value,
    verdict: &str,
    reasons: Vec<String>,
    left_events: usize,
    right_events: usize,
) -> Value {
    let left_summary = side_summary(left, left_document);
    let right_summary = side_summary(right, right_document);
    let signature_input = json!({
        "left":left_summary,
        "right":right_summary,
        "comparison":options,
    });
    let mut digest = Sha256::new();
    digest.update(COMPARISON_DOMAIN);
    digest.update(serde_json::to_vec(&signature_input).expect("comparison signature serializes"));
    json!({
        "schema_version":1,
        "kind":"wukong.trace.comparison",
        "comparison_signature":hex::encode(digest.finalize()),
        "verdict":verdict,
        "left":left_summary,
        "right":right_summary,
        "comparison":options,
        "counts":{"left_events":left_events,"right_events":right_events,"compared_events":0,"compared_captures":0,"differences":0},
        "differences_truncated":false,
        "differences":[],
        "reasons":reasons,
    })
}

fn side_summary(side: &ResolvedSide, document: Option<&TraceRunDocument>) -> Value {
    json!({
        "job_id":side.job.id,
        "run_evidence_id":side.run.id,
        "events_evidence_id":side.events.id,
        "run_sha256":side.run.sha256,
        "events_sha256":side.events.sha256,
        "source_target_sha256":side.job.target_sha256,
        "plan_signature":document.map(|document|document.plan_signature.clone()),
        "environment_signature":document.map(|document|document.launch["environment_signature"].clone()),
    })
}

fn publish(
    tools: &evidence::EvidenceService,
    agent: &AgentMetadata,
    call: &str,
    full: Value,
) -> Result<Value> {
    let bytes = serde_json::to_vec(&full)?;
    let sha256 = binary::sha256_bytes(&bytes);
    let existing = tools
        .store
        .records::<Evidence>(&agent.run_id)?
        .into_iter()
        .find(|record| {
            record.agent_id == agent.id
                && record.tool_call_id == call
                && record.source == "trace.comparison"
                && record.sha256 == sha256
        });
    let evidence = if let Some(existing) = existing {
        tools
            .store
            .verified_evidence_path(&agent.run_id, &existing.id)?;
        existing
    } else {
        tools.store.artifact(
            &agent.run_id,
            &agent.id,
            call,
            "trace.comparison",
            &bytes,
            "application/json",
            json!({
                "tool":"trace_compare",
                "verdict":full["verdict"],
                "comparison_signature":full["comparison_signature"],
                "left_run_sha256":full["left"]["run_sha256"],
                "right_run_sha256":full["right"]["run_sha256"],
                "left_events_sha256":full["left"]["events_sha256"],
                "right_events_sha256":full["right"]["events_sha256"],
            }),
        )?
    };
    let mut bounded = full;
    bounded["comparison_evidence_id"] = json!(evidence.id);
    bounded["full_comparison_bytes"] = json!(bytes.len());
    let mut omitted = 0_usize;
    let mut omitted_reasons = 0_usize;
    while serde_json::to_vec(&bounded)?.len().saturating_add(512) > tools.output_limit {
        if bounded["differences"]
            .as_array()
            .is_some_and(|differences| !differences.is_empty())
        {
            bounded["differences"].as_array_mut().expect("array").pop();
            omitted += 1;
        } else if bounded["reasons"]
            .as_array()
            .is_some_and(|reasons| !reasons.is_empty())
        {
            bounded["reasons"].as_array_mut().expect("array").pop();
            omitted_reasons += 1;
        } else {
            break;
        }
    }
    if omitted > 0 {
        bounded["differences_truncated"] = json!(true);
    }
    bounded["returned_differences"] = json!(bounded["differences"].as_array().map_or(0, Vec::len));
    bounded["differences_omitted_from_result"] = json!(omitted);
    bounded["reasons_omitted_from_result"] = json!(omitted_reasons);
    Ok(bounded)
}

fn truncate_reasons(reasons: &mut Vec<String>) {
    reasons.truncate(MAX_REASONS);
}

fn bounded_error(error: &dyn std::fmt::Display) -> String {
    error.to_string().chars().take(512).collect()
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
