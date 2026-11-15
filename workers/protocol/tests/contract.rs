use serde_json::Value;
use wukong_worker_protocol::{Method, contract, decode, validate_result};

#[test]
fn verification_expectations_do_not_add_absent_predicates() {
    for input in [
        serde_json::json!({"exit_code": 0}),
        serde_json::json!({"exit_code": 0, "signal": null, "stdout_contains": null, "stderr_contains": null}),
    ] {
        let expected: wukong_worker_protocol::params::Expected =
            serde_json::from_value(input).unwrap();
        assert_eq!(
            serde_json::to_value(expected).unwrap(),
            serde_json::json!({"exit_code": 0})
        );
    }
}

#[test]
fn shared_contract_corpus() {
    let samples: Vec<Value> =
        serde_json::from_str(include_str!("../fixtures/contract.json")).unwrap();
    let contract = contract();
    for sample in samples {
        let value = sample["value"].clone();
        let valid = match sample["kind"].as_str().unwrap() {
            "request" => decode(
                Method::parse(sample["method"].as_str().unwrap()).unwrap(),
                value,
            )
            .is_ok(),
            "result" => validate_result(
                Method::parse(sample["method"].as_str().unwrap()).unwrap(),
                &value,
            )
            .is_ok(),
            kind => jsonschema::is_valid(&contract[kind], &value),
        };
        assert_eq!(valid, sample["valid"].as_bool().unwrap(), "{sample}");
    }
}

#[test]
fn generated_contract_matches_checked_in_schema() {
    let checked: Value = serde_json::from_str(include_str!("../schema/contract.json")).unwrap();
    assert_eq!(checked, contract());
    for method in Method::ALL {
        assert_eq!(*method, Method::parse(method.name()).unwrap());
    }
    assert!(Method::parse("arbitrary.exec").is_err());
}
