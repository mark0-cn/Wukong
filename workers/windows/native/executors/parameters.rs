//! Decode RPC-boundary fields before constructing an execution plan.
use anyhow::{Context, Result, ensure};
use serde_json::{Map, Value};
pub(crate) fn request_id(params: &Map<String, Value>) -> Result<&str> {
    let request_id = params
        .get("request_id")
        .and_then(Value::as_str)
        .context("native request requires a stable request_id")?;
    ensure!(
        params
            .get("call_id")
            .is_none_or(|call| call.as_str() == Some(request_id)),
        "native call_id and request_id differ"
    );
    Ok(request_id)
}
pub(crate) fn clean(params: &Map<String, Value>) -> Map<String, Value> {
    let mut parameters = params.clone();
    for name in ["request_id", "call_id", "_binding", "_lease"] {
        parameters.remove(name);
    }
    parameters.retain(|_, value| !value.is_null());
    parameters
}
pub(crate) fn start_parameters<T: serde::de::DeserializeOwned>(
    params: &Map<String, Value>,
) -> Result<(&str, T)> {
    Ok((
        request_id(params)?,
        serde_json::from_value(Value::Object(clean(params)))?,
    ))
}
