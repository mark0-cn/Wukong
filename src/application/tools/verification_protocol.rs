//! Typed views over the existing worker wire format; original JSON stays evidence.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Deserialize)]
pub(super) struct ArtifactIdentity {
    pub sha256: String,
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
pub(super) struct NetworkBinding {
    pub launch_spec_sha256: String,
    pub challenge_id: String,
    pub target_sha256: String,
    pub service: Value,
    pub transport_host: String,
    pub transport_port: u16,
    pub runtime_identity_confirmed: bool,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize)]
pub(super) struct TrialObservation {
    pub input_sha256: String,
    pub network_binding: Option<NetworkBinding>,
    pub network_success: Option<bool>,
    pub transcript_path: Option<String>,
    pub transcript_sha256: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct VerificationResult {
    pub target: ArtifactIdentity,
    pub input: ArtifactIdentity,
    pub verifier_agent_id: String,
    pub producer_agent_id: String,
    pub call_id: String,
    pub fresh_working_directory_per_trial: bool,
    pub attempts: u64,
    pub trials: u64,
    pub requested_trials: u64,
    pub successes: u64,
    pub passed: bool,
    pub results: Vec<TrialObservation>,
    pub network_binding: Option<NetworkBinding>,
    pub fresh_connection_per_trial: Option<bool>,
    pub server_reset_per_trial: Option<bool>,
    pub expected: Value,
}

pub(super) enum VerificationPlan {
    Local,
    Network {
        steps: super::args::NetworkReplay,
        binding: NetworkBinding,
    },
}

impl VerificationPlan {
    pub fn parse(parameters: &Value) -> anyhow::Result<Self> {
        if parameters["network"].is_null() {
            Ok(Self::Local)
        } else {
            Ok(Self::Network {
                steps: serde_json::from_value(parameters["network"].clone())?,
                binding: serde_json::from_value(parameters["network_binding"].clone())?,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn network_identity_requires_typed_fields_and_retains_extensions() {
        let raw = json!({"launch_spec_sha256":"launch","challenge_id":"case","target_sha256":"target",
            "service":{"mode":"local_service"},"transport_host":"challenge","transport_port":1337,
            "runtime_identity_confirmed":false,"identity_limit":"not remote identity"});
        let binding: NetworkBinding = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(serde_json::to_value(binding).unwrap(), raw);
        let mut invalid = raw;
        invalid["transport_port"] = json!("1337");
        assert!(serde_json::from_value::<NetworkBinding>(invalid).is_err());
        assert!(VerificationPlan::parse(&json!({"network":{"steps":[]}})).is_err());
    }
}
