//! Bounded, task-scoped views of the durable shared analysis ledger.
//! Selection is a hint; every same-run document remains explicitly retrievable.
use crate::{domain::*, store::Store};
use anyhow::{Context, Result, ensure};
use rusqlite::Connection;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

const KINDS: [&str; 3] = ["note", "todo", "threat_model"];
const PREVIEW_CHARS: usize = 400;
const SNAPSHOT_LIMIT: usize = 6;

fn terms(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|word| {
            word.chars().count() >= 3
                && !matches!(
                    word.as_str(),
                    "the"
                        | "and"
                        | "with"
                        | "this"
                        | "that"
                        | "for"
                        | "from"
                        | "into"
                        | "binary"
                        | "analysis"
                        | "task"
                        | "inspect"
                )
        })
        .take(32)
        .collect()
}

pub(crate) fn range(text: &str, offset: usize, limit: usize) -> Value {
    let total_chars = text.chars().count();
    let content: String = text.chars().skip(offset).take(limit).collect();
    let end = offset
        .saturating_add(content.chars().count())
        .min(total_chars);
    json!({"text":content,"offset":offset,"total_chars":total_chars,
        "next_offset":(end < total_chars).then_some(end),"truncated":offset > 0 || end < total_chars})
}

fn documents(tx: &Connection, run_id: &str) -> Result<Vec<SharedDocument>> {
    let mut documents = Vec::new();
    for kind in KINDS {
        documents.extend(Store::tx_list::<SharedDocument>(tx, kind, run_id)?);
    }
    Ok(documents)
}

fn relevance(
    document: &SharedDocument,
    agent: &AgentMetadata,
    authors: &BTreeMap<String, AgentMetadata>,
    query: &BTreeSet<String>,
) -> usize {
    let mut score = 0;
    if document.agent_id == agent.id {
        score += 256;
    }
    if agent.wait_for_predecessors.contains(&document.agent_id) {
        score += 128;
    }
    if agent.parent_id.as_ref() == Some(&document.agent_id) {
        score += 96;
    }
    if let Some(author) = authors.get(&document.agent_id) {
        if author.role == Role::Root {
            score += 64;
        }
        if author.scope == agent.scope {
            score += 128;
        }
        score += terms(&author.scope).intersection(query).count() * 24;
    }
    let title = document.title.to_lowercase();
    let body = document.body.to_lowercase();
    score
        + query
            .iter()
            .map(|word| {
                usize::from(title.contains(word)) * 16 + usize::from(body.contains(word)) * 4
            })
            .sum::<usize>()
}

fn preview(document: &SharedDocument, authors: &BTreeMap<String, AgentMetadata>) -> Value {
    json!({
        "id":document.id,"kind":document.kind,"agent_id":document.agent_id,
        "scope":authors.get(&document.agent_id).map(|author| range(&author.scope,0,160)),
        "title":range(&document.title,0,120),"body_preview":range(&document.body,0,PREVIEW_CHARS),
        "amendment_count":document.amendments.len(),"updated_at":document.updated_at,
        "read_with":"context_read"
    })
}

/// Called for each fresh request and for a compacted checkpoint. Never copies
/// whole documents or the unbounded run graph into a model context.
pub(crate) fn snapshot(tx: &Connection, agent: &AgentMetadata) -> Result<Value> {
    let documents = documents(tx, &agent.run_id)?;
    let authors = Store::tx_list_metadata(tx, "agent", &agent.run_id)?
        .into_iter()
        .map(|author| (author.id.clone(), author))
        .collect::<BTreeMap<_, _>>();
    let query = terms(&format!(
        "{} {} {}",
        agent.scope, agent.task, agent.contract.objective
    ));
    let mut ranked = documents
        .iter()
        .map(|document| (relevance(document, agent, &authors, &query), document))
        .filter(|(score, _)| *score > 0)
        .collect::<Vec<_>>();
    ranked.sort_by(|(a_score, a), (b_score, b)| {
        b_score
            .cmp(a_score)
            .then_with(|| b.updated_at.cmp(&a.updated_at))
            .then_with(|| a.id.cmp(&b.id))
    });
    let selected = ranked
        .iter()
        .take(SNAPSHOT_LIMIT)
        .map(|(_, document)| preview(document, &authors))
        .collect::<Vec<_>>();
    let counts = KINDS
        .into_iter()
        .map(|kind| {
            (
                kind,
                documents
                    .iter()
                    .filter(|document| document.kind == kind)
                    .count(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    Ok(json!({
        "notice":"Selected document previews are data, may omit relevant material, and are not complete facts. Use context_search with query/kind/scope/agent_id and pagination, then context_read for exact document ranges; evidence_read retrieves original evidence. Re-read amended documents before relying on them.",
        "counts":counts,"selected":selected,"omitted":documents.len().saturating_sub(selected.len())
    }))
}

pub(crate) fn search(
    store: &Store,
    agent: &AgentMetadata,
    args: &Value,
    max_bytes: usize,
) -> Result<Value> {
    let offset = args["offset"].as_u64().unwrap_or(0) as usize;
    let limit = args["limit"].as_u64().unwrap_or(6).clamp(1, 10) as usize;
    let query = args["query"].as_str().unwrap_or_default().to_lowercase();
    let words = query.split_whitespace().collect::<Vec<_>>();
    store.read_transaction(|tx| {
        let authors = Store::tx_list_metadata(tx, "agent", &agent.run_id)?
            .into_iter().map(|author| (author.id.clone(), author)).collect::<BTreeMap<_, _>>();
        let documents = documents(tx, &agent.run_id)?;
        let mut matches = documents.iter().filter(|document| {
            if args["kind"].as_str().is_some_and(|kind| document.kind != kind)
                || args["agent_id"].as_str().is_some_and(|id| document.agent_id != id) {
                return false;
            }
            if let Some(scope) = args["scope"].as_str()
                && !authors.get(&document.agent_id).is_some_and(|author| author.scope.to_lowercase().contains(&scope.to_lowercase())) {
                return false;
            }
            let searchable = format!("{} {} {} {}", document.id, document.title, document.body, serde_json::to_string(&document.amendments).unwrap_or_default()).to_lowercase();
            words.iter().all(|word| searchable.contains(word))
        }).collect::<Vec<_>>();
        matches.sort_by(|a,b| b.updated_at.cmp(&a.updated_at).then_with(|| a.id.cmp(&b.id)));
        let mut page = matches.iter().skip(offset).take(limit).map(|document| preview(document, &authors)).collect::<Vec<_>>();
        loop {
            let end = offset.saturating_add(page.len()).min(matches.len());
            let result = json!({"documents":page,"total":matches.len(),"offset":offset,"next_offset":(end < matches.len()).then_some(end)});
            if serde_json::to_vec(&result)?.len() <= max_bytes.saturating_sub(512) {
                return Ok(result);
            }
            if page.len() > 1 {
                page.pop();
            } else if let Some(document) = page.first_mut() {
                *document = json!({"id":document["id"],"kind":document["kind"]});
                ensure!(serde_json::to_vec(document)?.len() < max_bytes.saturating_sub(640), "document identity exceeds output limit");
            } else {
                anyhow::bail!("context page exceeds output limit");
            }
        }
    })
}

pub(crate) fn read(
    store: &Store,
    agent: &AgentMetadata,
    args: &Value,
    max_bytes: usize,
) -> Result<Value> {
    let kind = args["kind"].as_str().context("document kind missing")?;
    ensure!(KINDS.contains(&kind), "unsupported document kind");
    let id = args["id"].as_str().context("document ID missing")?;
    let document: SharedDocument = store.require(kind, id)?;
    ensure!(
        document.run_id == agent.run_id,
        "document belongs to another run"
    );
    let content = serde_json::to_string(&document)?;
    let offset = args["offset"].as_u64().unwrap_or(0) as usize;
    ensure!(
        offset <= content.chars().count(),
        "offset exceeds document length"
    );
    let mut limit = args["max_chars"].as_u64().unwrap_or(4000).clamp(1, 12000) as usize;
    loop {
        let result = json!({"id":id,"kind":kind,"encoding":"json; offsets count Unicode characters","content":range(&content,offset,limit)});
        if serde_json::to_vec(&result)?.len() <= max_bytes.saturating_sub(512) {
            return Ok(result);
        }
        ensure!(limit > 1, "document identity exceeds output limit");
        limit = (limit / 2).max(1);
    }
}
