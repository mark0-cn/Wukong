//! Typed native operations mapped to the pinned external MCP tools. Upstream
//! owns debugger syntax, execution handles, waits and interruption.
use crate::{
    debugger_mcp::Client,
    session_wire::{Action, Control, Inspect, Inspection},
};
use anyhow::{Context, Result, ensure};
use rmcp::model::CallToolResult;
use serde::{Deserialize, Deserializer, de::Error};
use serde_json::{Value, json};

pub(crate) fn default_timeout() -> u32 {
    30
}
pub(crate) fn inspection<'de, D: Deserializer<'de>>(input: D) -> Result<i32, D::Error> {
    let name = String::deserialize(input)?;
    let value = Inspection::from_str_name(&name.to_ascii_uppercase())
        .filter(|value| {
            value.as_str_name().to_ascii_lowercase() == name && *value != Inspection::Unspecified
        })
        .ok_or_else(|| D::Error::custom("unsupported debugger inspection"))?;
    Ok(value as i32)
}
pub(crate) fn action<'de, D: Deserializer<'de>>(input: D) -> Result<i32, D::Error> {
    let name = String::deserialize(input)?;
    let value = Action::from_str_name(&name.to_ascii_uppercase())
        .filter(|value| {
            value.as_str_name().to_ascii_lowercase() == name && *value != Action::Unspecified
        })
        .ok_or_else(|| D::Error::custom("unsupported debugger action"))?;
    Ok(value as i32)
}
pub(crate) fn breakpoint_id<'de, D: Deserializer<'de>>(input: D) -> Result<Option<u32>, D::Error> {
    Option::<String>::deserialize(input)?
        .map(|name| {
            let id: u32 = name.parse().map_err(D::Error::custom)?;
            if id.to_string() != name {
                return Err(D::Error::custom("breakpoint ID must be canonical decimal"));
            }
            Ok(id)
        })
        .transpose()
}
impl Inspect {
    pub(crate) fn validate(&self) -> Result<Inspection> {
        let kind = Inspection::try_from(self.kind)?;
        ensure!(
            kind != Inspection::Unspecified && (1..=60).contains(&self.timeout_seconds),
            "invalid debugger inspection or timeout"
        );
        ensure!(
            self.length.is_none_or(|n| (1..=65536).contains(&n))
                && self.limit.is_none_or(|n| (1..=256).contains(&n)),
            "debugger inspection exceeds its bounds"
        );
        ensure!(
            self.address.is_none() || matches!(kind, Inspection::Memory | Inspection::Disassembly),
            "inspection does not accept an address"
        );
        ensure!(
            self.length.is_none() || kind == Inspection::Memory,
            "inspection does not accept a byte length"
        );
        ensure!(
            self.limit.is_none()
                || matches!(
                    kind,
                    Inspection::Stack
                        | Inspection::Modules
                        | Inspection::Disassembly
                        | Inspection::Exception
                ),
            "inspection does not accept a record limit"
        );
        ensure!(
            kind != Inspection::Memory || self.address.is_some(),
            "memory inspection requires an address"
        );
        ensure!(
            !matches!(kind, Inspection::Disassembly | Inspection::Exception)
                || self.limit.is_none_or(|n| n <= 128),
            "upstream disassembly and exception triage support at most 128 records"
        );
        Ok(kind)
    }
}

impl Control {
    pub(crate) fn validate(&self) -> Result<Action> {
        let action = Action::try_from(self.action)?;
        ensure!(
            action != Action::Unspecified && (1..=60).contains(&self.timeout_seconds),
            "invalid debugger action or timeout"
        );
        match action {
            Action::BreakpointAdd => ensure!(
                (self.address.is_some() ^ self.symbol.is_some()) && self.breakpoint_id.is_none(),
                "breakpoint add requires exactly one address or symbol"
            ),
            Action::BreakpointRemove | Action::BreakpointEnable | Action::BreakpointDisable => {
                ensure!(
                    self.breakpoint_id.is_some() && self.address.is_none() && self.symbol.is_none(),
                    "breakpoint action requires only its numeric ID"
                )
            }
            _ => ensure!(
                self.address.is_none() && self.symbol.is_none() && self.breakpoint_id.is_none(),
                "execution action does not accept breakpoint arguments"
            ),
        }
        if let Some(symbol) = &self.symbol {
            ensure!(
                !symbol.is_empty() && symbol.len() <= 512 && !symbol.chars().any(char::is_control),
                "invalid debugger symbol length or control characters"
            );
        }
        Ok(action)
    }
}

pub(crate) struct Session {
    client: Client,
    id: String,
    execution: Option<String>,
}
impl Session {
    pub(crate) fn new(client: Client, id: String) -> Self {
        Self {
            client,
            id,
            execution: None,
        }
    }
    pub(crate) async fn inspect(&mut self, request: Inspect) -> Result<Value> {
        let kind = request.validate()?;
        let limit = request.limit.unwrap_or(32);
        let (tool, mut args) = match kind {
            Inspection::Registers => ("registers", json!({})),
            Inspection::Stack => ("backtrace", json!({"frames":limit})),
            Inspection::Memory => (
                "read_memory",
                json!({"address":format!("0x{:x}",request.address.unwrap()),"size":request.length.unwrap_or(64)}),
            ),
            Inspection::Modules => ("modules", json!({"limit":limit})),
            Inspection::Threads => ("threads", json!({})),
            Inspection::Disassembly => (
                "disassemble",
                match request.address {
                    Some(a) => {
                        json!({"address":format!("0x{a:x}"),"count":request.limit.unwrap_or(16)})
                    }
                    None => json!({"count":request.limit.unwrap_or(16)}),
                },
            ),
            Inspection::Exception => (
                "exception_triage",
                json!({"frames":limit,"scan_stack":false}),
            ),
            Inspection::State => {
                let status = self
                    .client
                    .raw("session_status", json!({"session_id":self.id}))
                    .await?;
                let execution = match &self.execution {
                    Some(handle) => Some(
                        self.client
                            .raw(
                                "wait_for_stop",
                                json!({"session_id":self.id,"execution":handle,"timeout_ms":0}),
                            )
                            .await?,
                    ),
                    None => None,
                };
                return Ok(json!({"status":status,"execution":execution}));
            }
            Inspection::Unspecified => unreachable!(),
        };
        args["session_id"] = json!(self.id);
        Ok(serde_json::to_value(self.client.raw(tool, args).await?)?)
    }
    pub(crate) async fn control(&mut self, request: Control) -> Result<Value> {
        let action = request.validate()?;
        let (tool, mut args) = match action {
            Action::Run | Action::Continue => (
                "continue_async",
                json!({"max_run_ms":request.timeout_seconds*1000}),
            ),
            Action::Interrupt => match &self.execution {
                Some(handle) => ("break_in", json!({"execution":handle})),
                None => return Ok(json!({"already_stopped":true})),
            },
            Action::Step => ("step_into", json!({})),
            Action::Next => ("step_over", json!({})),
            Action::Finish => ("execute", json!({"command":"gu"})),
            Action::BreakpointAdd => (
                "set_breakpoint",
                json!({"expression":request.symbol.unwrap_or_else(||format!("0x{:x}",request.address.unwrap()))}),
            ),
            Action::BreakpointRemove => (
                "clear_breakpoints",
                json!({"ids":[request.breakpoint_id.unwrap()]}),
            ),
            // The pinned tool has no dedicated toggle/step-out endpoints. Only
            // these fixed commands and validated integers reach its passthrough.
            Action::BreakpointEnable => (
                "execute",
                json!({"command":format!("be {}",request.breakpoint_id.unwrap())}),
            ),
            Action::BreakpointDisable => (
                "execute",
                json!({"command":format!("bd {}",request.breakpoint_id.unwrap())}),
            ),
            Action::Unspecified => unreachable!(),
        };
        args["session_id"] = json!(self.id);
        let result: CallToolResult = self.client.raw(tool, args).await?;
        if result.is_error != Some(true) {
            if matches!(action, Action::Run | Action::Continue) {
                let data = result
                    .structured_content
                    .as_ref()
                    .context("upstream run receipt is not structured")?;
                ensure!(
                    data["session_id"] == self.id,
                    "upstream run belongs to another session"
                );
                let execution = data["execution"]
                    .as_str()
                    .context("upstream run handle missing")?;
                ensure!(
                    !execution.is_empty() && execution.len() <= 128,
                    "invalid upstream run handle"
                );
                self.execution = Some(execution.into());
            } else if matches!(action, Action::Step | Action::Next | Action::Finish) {
                self.execution = None;
            }
        }
        Ok(serde_json::to_value(result)?)
    }
    pub(crate) async fn close(mut self) -> Result<Value> {
        let result = self
            .client
            .structured("end_session", json!({"session_id":self.id}))
            .await;
        let closed = self.client.close().await;
        let result = result?;
        closed?;
        ensure!(
            result["released"] == true
                && result["worker_terminated"] == true
                && result["recovery_required"] == false,
            "upstream did not confirm session release"
        );
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn typed_parameters_reject_ambiguous_or_unbounded_operations() -> Result<()> {
        for value in [
            json!({"action":"breakpoint_enable","breakpoint_id":"0;g"}),
            json!({"action":"breakpoint_remove","breakpoint_id":"01"}),
            json!({"action":"breakpoint_add","symbol":"main","address":1}),
            json!({"action":"breakpoint_add"}),
            json!({"action":"continue","symbol":"main"}),
            json!({"action":"execute","command":"g"}),
            json!({"action":"step","timeout_secs":61}),
            json!({"action":"breakpoint_enable","breakpoint_id":1}),
        ] {
            assert!(
                serde_json::from_value::<Control>(value.clone())
                    .map_err(anyhow::Error::from)
                    .and_then(|v| v.validate())
                    .is_err(),
                "accepted {value}"
            );
        }
        for value in [
            json!({"kind":"memory"}),
            json!({"kind":"memory","address":0,"length":65537}),
            json!({"kind":"disassembly","limit":129}),
            json!({"kind":"registers","address":1}),
            json!({"kind":"state","limit":2}),
            json!({"kind":"threads","length":2}),
            json!({"kind":"stack","timeout_secs":0}),
            json!({"kind":"REGISTERS"}),
        ] {
            assert!(
                serde_json::from_value::<Inspect>(value.clone())
                    .map_err(anyhow::Error::from)
                    .and_then(|v| v.validate())
                    .is_err(),
                "accepted {value}"
            );
        }
        let bp: Control =
            serde_json::from_value(json!({"action":"breakpoint_enable","breakpoint_id":"0"}))?;
        assert_eq!(bp.validate()?, Action::BreakpointEnable);
        assert_eq!(bp.timeout_seconds, 30);
        let memory: Inspect =
            serde_json::from_value(json!({"kind":"memory","address":u64::MAX,"length":65536}))?;
        assert_eq!(memory.validate()?, Inspection::Memory);
        Ok(())
    }
}
