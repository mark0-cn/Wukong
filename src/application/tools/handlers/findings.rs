//! Findings tool handlers.
use super::*;

pub(in crate::application::tools) fn handle_finding_file<'a>(
    tools: &'a evidence::EvidenceService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { tools.finding(agent, call_id, name, &args) })
}

pub(in crate::application::tools) fn handle_findings_list<'a>(
    tools: &'a evidence::EvidenceService,
    agent: &'a AgentMetadata,
    _call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { super::super::inventory::findings(&tools.store, agent, &args) })
}

pub(in crate::application::tools) fn handle_finding_get<'a>(
    tools: &'a evidence::EvidenceService,
    agent: &'a AgentMetadata,
    _call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        let finding: Finding = require_in_run(&tools.store, text(&args, "id")?, &agent.run_id)?;
        let verification = finding
            .verification_id
            .as_ref()
            .map(|id| require_in_run::<Verification>(&tools.store, id, &agent.run_id))
            .transpose()?;
        Ok(json!({"finding":finding,"verification":verification}))
    })
}

pub(in crate::application::tools) fn handle_verification_record<'a>(
    tools: &'a ExecutionService,
    agent: &'a AgentMetadata,
    call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move { tools.verification.record(agent, call_id, &args).await })
}

fn complete_character_end(bytes: &[u8], start: usize) -> Option<usize> {
    (1..=4).find_map(|length| {
        let end = start.checked_add(length)?;
        let candidate = bytes.get(start..end)?;
        std::str::from_utf8(candidate)
            .ok()
            .filter(|text| text.chars().count() == 1)
            .map(|_| end)
    })
}

/// Return valid UTF-8 boundaries for a requested window. The buffer may contain
/// three bytes before and after the request, allowing us to distinguish a split
/// code point from genuinely invalid bytes without changing artifact addressing.
fn utf8_window(bytes: &[u8], requested: std::ops::Range<usize>) -> Option<std::ops::Range<usize>> {
    if std::str::from_utf8(&bytes[requested.clone()]).is_ok() {
        return Some(requested);
    }
    let mut start = requested.start;
    if bytes.get(start).is_some_and(|byte| byte & 0xc0 == 0x80) {
        while bytes.get(start).is_some_and(|byte| byte & 0xc0 == 0x80) {
            start += 1;
        }
        let mut lead = requested.start;
        while lead > 0 && bytes[lead - 1] & 0xc0 == 0x80 {
            lead -= 1;
        }
        lead = lead.checked_sub(1)?;
        let prefix = bytes.get(lead..start)?;
        if lead >= requested.start
            || std::str::from_utf8(prefix)
                .ok()
                .is_none_or(|text| text.chars().count() != 1)
        {
            return None;
        }
    }
    if start >= requested.end {
        return complete_character_end(bytes, start).map(|end| start..end);
    }
    match std::str::from_utf8(&bytes[start..requested.end]) {
        Ok(_) => Some(start..requested.end),
        Err(error) if error.error_len().is_some() => None,
        Err(error) => {
            let incomplete = start + error.valid_up_to();
            let character_end = complete_character_end(bytes, incomplete)?;
            if incomplete == start {
                Some(start..character_end)
            } else {
                Some(start..incomplete)
            }
        }
    }
}
pub(in crate::application::tools) fn handle_evidence_read<'a>(
    tools: &'a evidence::EvidenceService,
    agent: &'a AgentMetadata,
    _call_id: &'a str,
    _name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        let evidence_id = text(&args, "evidence_id")?.to_owned();
        let record: Evidence = require_in_run(&tools.store, &evidence_id, &agent.run_id)?;
        let total = usize::try_from(record.size)?;
        let offset = usize::try_from(number(&args, "offset", 0))?;
        ensure!(offset <= total, "evidence offset is past end of file");
        let length = usize::try_from(number(&args, "length", 8192))?.min(tools.read_limit);
        let requested_length = length.min(total - offset);
        let prefix_length = offset.min(3);
        let read_start = offset - prefix_length;
        let read_length = prefix_length
            .saturating_add(requested_length)
            .saturating_add(3)
            .min(total - read_start);
        let bytes = if read_length == 0 {
            Vec::new()
        } else {
            tools.store.read_evidence_range(
                &agent.run_id,
                &evidence_id,
                read_start as u64,
                read_length,
            )?
        };
        let requested = prefix_length..prefix_length + requested_length;
        let original = &bytes[requested.clone()];
        let (range, utf8) = utf8_window(&bytes, requested.clone())
            .map_or((requested.clone(), false), |range| (range, true));
        let actual_offset = read_start + range.start;
        let end = read_start + range.end;
        let content = if utf8 {
            json!({"encoding":"utf8","data":std::str::from_utf8(&bytes[range.clone()])?})
        } else {
            json!({"encoding":"base64","data":BASE64.encode(original)})
        };
        let mut result = json!({"evidence_id":evidence_id,"offset":actual_offset,"length":range.len(),"total_bytes":total,"next_offset":if end<total{Some(end)}else{None},"content":content});
        if range != requested {
            result["boundary_adjusted"] = json!(true);
            result["requested_offset"] = json!(offset);
            result["requested_length"] = json!(requested_length);
        }
        Ok(result)
    })
}

pub(in crate::application::tools) fn handle_inventory<'a>(
    tools: &'a evidence::EvidenceService,
    agent: &'a AgentMetadata,
    _call: &'a str,
    name: &'a str,
    args: Value,
) -> BoxFuture<'a, Result<Value>> {
    Box::pin(async move {
        match name {
            "evidence_search" => super::super::inventory::evidence(&tools.store, agent, &args),
            _ => super::super::inventory::resources(&tools.store, agent, &args),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::utf8_window;

    #[test]
    fn utf8_windows_split_only_at_character_boundaries() {
        let bytes = "a界b".as_bytes();
        assert_eq!(utf8_window(bytes, 0..2), Some(0..1));
        assert_eq!(utf8_window(bytes, 1..2), Some(1..4));
        assert_eq!(utf8_window(bytes, 2..3), Some(4..5));
        assert_eq!(utf8_window(bytes, 3..4), Some(4..5));
        assert_eq!(utf8_window(bytes, 4..5), Some(4..5));
    }

    #[test]
    fn invalid_utf8_is_not_reinterpreted_as_a_boundary_split() {
        assert_eq!(utf8_window(b"\x80a", 0..2), None);
        assert_eq!(utf8_window(b"a\xffb", 0..3), None);
        assert_eq!(utf8_window(b"a\xe7\x95", 0..3), None);
        assert_eq!(utf8_window(b"\x80\x80\x80\x80a", 1..5), None);
    }
}
