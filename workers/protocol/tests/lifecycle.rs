use serde_json::Value;
use wukong_worker_protocol::{JobStatus, OutcomeCertainty, SessionObservation};

#[test]
fn shared_invalid_lifecycle_fields_are_rejected() {
    let corpus: Value = serde_json::from_str(include_str!("../fixtures/lifecycle.json")).unwrap();
    for case in corpus["invalid_observations"].as_array().unwrap() {
        let rejected = if case["kind"] == "job" {
            serde_json::from_value::<JobStatus>(case["wire"].clone()).is_err()
        } else {
            serde_json::from_value::<SessionObservation>(case["wire"].clone()).is_err()
        };
        assert!(rejected, "{}", case["name"]);
    }
}

#[test]
fn shared_lifecycle_observations_preserve_completion_and_uncertainty() {
    let corpus: Value = serde_json::from_str(include_str!("../fixtures/lifecycle.json")).unwrap();
    for case in corpus["observations"].as_array().unwrap() {
        let observation = if case["kind"] == "job" {
            serde_json::from_value::<JobStatus>(case["wire"].clone())
                .unwrap()
                .observation()
        } else {
            serde_json::from_value::<SessionObservation>(case["wire"].clone())
                .unwrap()
                .observation()
        };
        assert_eq!(
            observation.settled(),
            case["settled"].as_bool().unwrap(),
            "{}",
            case["name"]
        );
        assert_eq!(
            observation.outcome == OutcomeCertainty::Unknown,
            case["outcome"] == "unknown",
            "{}",
            case["name"]
        );
    }
}

#[test]
fn lifecycle_fields_are_typed_without_losing_extension_evidence() {
    let raw = serde_json::json!({"job_id":"fixture","state":"lost","cleanup_pending":true,
        "result":{"outcome":"unknown","jobs_empty":false,"vendor_observation":{"raw":42}},"vendor":"fixture"});
    let status: JobStatus = serde_json::from_value(raw).unwrap();
    assert!(!status.settled());
    let result = serde_json::to_value(status).unwrap();
    assert_eq!(result["result"]["vendor_observation"]["raw"], 42);
    assert_eq!(result["vendor"], "fixture");
    for value in [serde_json::json!("false"), serde_json::json!(0)] {
        assert!(
            serde_json::from_value::<JobStatus>(
                serde_json::json!({"job_id":"fixture","state":"completed","cleanup_pending":value})
            )
            .is_err()
        );
    }
}
