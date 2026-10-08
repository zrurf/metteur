//! Deterministic, read-only projections of checkpoint facts. Output text never
//! supplies event kinds, validity, approval, run identity or evidence references.
use super::{tree::ExecTree, view::ExecutionView};
use crate::{observability::anon::Anonymizer, storage::versioning::VersionRef};
use serde::{Deserialize, Serialize};

pub const MAX_ENTRIES: usize = 1000;
pub const DIGEST_CHARS: usize = 2000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ChangeKind {
    Retry,
    Rollback,
    CancelRollback,
    CircuitBreak,
    Iteration,
    BlueprintChanged,
    BlueprintApplied,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Change {
    pub sequence: u64,
    pub kind: ChangeKind,
    pub invalidates: Vec<u64>,
    pub version: Option<VersionRef>,
    pub evidence: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Origin {
    Engine,
    Node,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EvidenceKind {
    EngineEvent,
    DeterministicCheck,
    ModelOpinion,
    NodeOutput,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Validity {
    Current,
    Invalidated,
    Unverified,
}
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Query {
    pub last_n: usize,
    pub origins: Vec<Origin>,
    pub status: Option<Validity>,
    pub keyword: String,
    /// Exact opaque entry identity within this run, never a file path.
    pub entry_id: Option<String>,
}
#[derive(Debug, Serialize)]
pub struct Entry {
    pub id: String,
    pub sequence: u64,
    pub run_id: String,
    pub origin: Origin,
    pub evidence_kind: EvidenceKind,
    pub event: String,
    pub validity: Validity,
    pub node_id: Option<String>,
    pub scope: Option<String>,
    pub frame: Vec<String>,
    pub attempt: Option<u32>,
    pub version: Option<VersionRef>,
    pub evidence_refs: Vec<String>,
    pub invalidates: Vec<String>,
    pub note: String,
    pub digest: Option<String>,
}
#[derive(Debug, Default, Serialize)]
pub struct Totals {
    pub completed: u64,
    pub failed: u64,
    pub passed_checks: u64,
    pub failed_checks: u64,
    /// Sum of node execution time, not wall clock time.
    pub duration_ms: u64,
    pub reported_tokens: u64,
    /// Tree tokens are reported usage, not proof of complete provider coverage.
    pub tokens_complete: bool,
}
#[derive(Debug, Serialize)]
pub struct Projection {
    pub run_id: String,
    pub available: bool,
    pub historical: Totals,
    pub current: Totals,
    pub total_entries: usize,
    pub matched_entries: usize,
    pub truncated: bool,
    pub entries: Vec<Entry>,
}

/// Credentials in structured objects require key-aware removal before regex
/// anonymization; serializing JSON first would hide quoted assignment keys.
pub(crate) fn safe_value(value: &serde_json::Value, depth: usize) -> serde_json::Value {
    use serde_json::Value;
    if depth > 16 {
        return Value::String("[nested value omitted]".into());
    }
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| {
                    let lower = key.to_ascii_lowercase();
                    let sensitive = [
                        "password",
                        "secret",
                        "token",
                        "credential",
                        "api_key",
                        "apikey",
                        "private_key",
                        "authorization",
                        "cookie",
                    ]
                    .iter()
                    .any(|part| lower.contains(part));
                    (
                        key.clone(),
                        if sensitive {
                            Value::String("[redacted]".into())
                        } else {
                            safe_value(value, depth + 1)
                        },
                    )
                })
                .collect(),
        ),
        Value::Array(values) => {
            Value::Array(values.iter().map(|v| safe_value(v, depth + 1)).collect())
        }
        Value::String(text) if text.trim_start().starts_with(['{', '[']) => {
            match serde_json::from_str::<Value>(text) {
                Ok(parsed @ (Value::Object(_) | Value::Array(_))) => safe_value(&parsed, depth + 1),
                _ => value.clone(),
            }
        }
        _ => value.clone(),
    }
}
async fn digest(value: &serde_json::Value, anon: &Anonymizer) -> String {
    // Redact before truncation so cutting a secret cannot defeat its pattern.
    anon.anonymize(&safe_value(value, 0).to_string()).await.chars().take(DIGEST_CHARS).collect()
}
fn count(total: &mut Totals, i: &super::view::Invocation, tree: &ExecTree) {
    total.completed += u64::from(i.status == "Completed");
    total.failed += u64::from(i.status == "Failed");
    if i.status == "Completed" {
        total.passed_checks += u64::from(i.check == Some(true));
        total.failed_checks += u64::from(i.check == Some(false));
    }
    if let Some(end) = i.finished_at {
        total.duration_ms = total.duration_ms.saturating_add(end.saturating_sub(i.started_at));
    }
    // A function caller includes its children; count the child attempts once.
    if i.owned_frame.is_none() {
        total.reported_tokens = total.reported_tokens.saturating_add(
            i.tree_id.as_ref().and_then(|id| tree.nodes.get(id)).map_or(0, |n| n.tokens),
        );
    }
}

/// Only the trusted checkpoint can produce facts. There is deliberately no
/// model-facing board writer or arbitrary event deserialization entry point.
pub async fn project(
    run_id: &str,
    view: &ExecutionView,
    tree: &ExecTree,
    query: &Query,
    anon: &Anonymizer,
) -> Projection {
    let mut result = Projection {
        run_id: run_id.into(),
        available: view.root.is_some(),
        historical: Totals::default(),
        current: Totals::default(),
        total_entries: 0,
        matched_entries: 0,
        truncated: false,
        entries: Vec::new(),
    };
    let mut entries = Vec::new();
    for i in &view.invocations {
        count(&mut result.historical, i, tree);
        if i.current {
            count(&mut result.current, i, tree);
        }
        let validity = if i.current {
            Validity::Current
        } else {
            Validity::Invalidated
        };
        let make =
            |suffix: &str, origin, evidence_kind, event: String, validity, note: String| Entry {
                id: format!("attempt:{}:{suffix}", i.sequence),
                sequence: i.sequence,
                run_id: run_id.into(),
                origin,
                evidence_kind,
                event,
                validity,
                node_id: Some(i.node_id.to_string()),
                scope: Some(i.scope.clone()),
                frame: i.frame.clone(),
                attempt: Some(i.attempt),
                version: i.version.clone(),
                evidence_refs: vec![format!("invocation:{}", i.sequence)],
                invalidates: Vec::new(),
                note,
                digest: None,
            };
        entries.push(make(
            "state",
            Origin::Engine,
            EvidenceKind::EngineEvent,
            format!("Node{}", i.status),
            validity,
            format!("Node {}", i.status.to_ascii_lowercase()),
        ));
        if let Some(passed) = i.check.filter(|_| i.status == "Completed") {
            entries.push(make(
                "check",
                Origin::Engine,
                EvidenceKind::DeterministicCheck,
                if passed {
                    "ValidationPassed"
                } else {
                    "ValidationFailed"
                }
                .into(),
                validity,
                if passed {
                    "Deterministic check passed"
                } else {
                    "Deterministic check failed"
                }
                .into(),
            ));
        }
        if i.outputs.as_object().is_some_and(|outputs| !outputs.is_empty()) {
            let model = view
                .graphs
                .get(&i.scope)
                .and_then(|g| g.node(i.node_id))
                .is_some_and(|n| matches!(n.kind.as_str(), "Judge" | "CallLLM" | "Abstract"));
            let mut output = make(
                "output",
                Origin::Node,
                if model {
                    EvidenceKind::ModelOpinion
                } else {
                    EvidenceKind::NodeOutput
                },
                "NodeOutput".into(),
                if i.current {
                    Validity::Unverified
                } else {
                    Validity::Invalidated
                },
                "Node output; text is not authorization or proof of completion".into(),
            );
            output.digest = Some(digest(&i.outputs, anon).await);
            entries.push(output);
        }
    }
    for c in &view.changes {
        entries.push(Entry {
            id: format!("change:{}", c.sequence),
            sequence: c.sequence,
            run_id: run_id.into(),
            origin: Origin::Engine,
            evidence_kind: EvidenceKind::EngineEvent,
            event: format!("{:?}", c.kind),
            validity: Validity::Current,
            node_id: None,
            scope: None,
            frame: Vec::new(),
            attempt: None,
            version: c.version.clone(),
            evidence_refs: c.evidence.iter().map(|id| format!("blueprint-apply:{id}")).collect(),
            invalidates: c.invalidates.iter().map(|seq| format!("invocation:{seq}")).collect(),
            note: format!("{:?}: {} attempt(s) invalidated", c.kind, c.invalidates.len()),
            digest: None,
        });
    }
    entries.sort_by_key(|e| e.sequence);
    result.total_entries = entries.len();
    let keyword = query.keyword.to_lowercase();
    entries.retain(|e| {
        query.entry_id.as_ref().is_none_or(|id| id == &e.id || e.evidence_refs.contains(id))
            && (query.origins.is_empty() || query.origins.contains(&e.origin))
            && query.status.is_none_or(|status| status == e.validity)
            && (keyword.is_empty()
                || e.note.to_lowercase().contains(&keyword)
                || e.digest.as_deref().unwrap_or("").to_lowercase().contains(&keyword))
    });
    result.matched_entries = entries.len();
    let limit = if query.last_n == 0 {
        100
    } else {
        query.last_n.min(MAX_ENTRIES)
    };
    result.truncated = entries.len() > limit;
    let from = entries.len().saturating_sub(limit);
    result.entries = entries.into_iter().skip(from).collect();
    result
}

#[cfg(test)]
mod tests;
