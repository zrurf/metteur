//! Replanning: user-approved edits to the executing blueprint.
//!
//! Local ReplanBlueprint tools and the bounded supervisor share the same
//! versioned application service. The interpreter owns circuit review and retry;
//! the legacy PlanAgent helper only produces edits and grants no authority.

use metteur_shared::Blueprint;
use metteur_shared::llm::ContextManager;

use crate::error::{DaemonError, DaemonResult};
use crate::execution::context::ExecutionContext;
use crate::execution::react::{ReactOptions, run_react};
use crate::sandbox::approval::Decision;

/// One edit operation; parsed from `{ "op": ..., "match": ..., ... }` JSON.
enum EditOp {
    /// Replace `node.data` wholesale.
    SetData {
        selector: NodeSelector,
        data: serde_json::Value,
    },
    /// Replace one key inside `node.data` (a pin default value).
    SetPin {
        selector: NodeSelector,
        key: String,
        value: serde_json::Value,
    },
}

/// Selects a node by `{"kind": ..., "nth": N}` (1-based occurrence).
#[derive(Debug)]
struct NodeSelector {
    kind: String,
    nth: usize,
}

impl NodeSelector {
    fn find_index(&self, bp: &Blueprint) -> Option<usize> {
        let skip = self.nth.checked_sub(1)?;
        bp.nodes.iter().enumerate().filter(|(_, n)| n.kind == self.kind).nth(skip).map(|(i, _)| i)
    }
}

/// Parses `edits` (a JSON array) into typed operations.
fn parse_edits(script: &serde_json::Value) -> Result<Vec<EditOp>, String> {
    let edits = script.as_array().ok_or_else(|| "edits must be a JSON array".to_string())?;
    if edits.is_empty() {
        return Err("edits is empty".to_string());
    }
    edits.iter().map(parse_edit).collect()
}

fn parse_edit(edit: &serde_json::Value) -> Result<EditOp, String> {
    let obj = edit.as_object().ok_or_else(|| "each edit must be a JSON object".to_string())?;
    let op =
        obj.get("op").and_then(|v| v.as_str()).ok_or_else(|| "edit missing 'op'".to_string())?;
    let selector = parse_selector(obj.get("match"))?;
    match op {
        "set_data" => {
            let data =
                obj.get("data").cloned().ok_or_else(|| "set_data requires 'data'".to_string())?;
            Ok(EditOp::SetData {
                selector,
                data,
            })
        }
        "set_pin" => {
            let key = obj
                .get("pin")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "set_pin requires 'pin'".to_string())?
                .to_string();
            let value =
                obj.get("value").cloned().ok_or_else(|| "set_pin requires 'value'".to_string())?;
            Ok(EditOp::SetPin {
                selector,
                key,
                value,
            })
        }
        other => Err(format!("unknown edit op '{other}'")),
    }
}

fn parse_selector(value: Option<&serde_json::Value>) -> Result<NodeSelector, String> {
    let obj = value
        .and_then(|v| v.as_object())
        .ok_or_else(|| "edit missing 'match' object".to_string())?;
    let kind = obj
        .get("kind")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "match.kind is required".to_string())?
        .to_string();
    let nth = obj.get("nth").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
    Ok(NodeSelector {
        kind,
        nth: nth.max(1),
    })
}

/// Applies the edit script to `bp`, returning a short human summary.
pub fn apply_edits(bp: &mut Blueprint, script: &serde_json::Value) -> Result<String, String> {
    let mut candidate = bp.clone();
    let summary = apply_edits_inner(&mut candidate, script)?;
    *bp = candidate;
    Ok(summary)
}

fn apply_edits_inner(bp: &mut Blueprint, script: &serde_json::Value) -> Result<String, String> {
    let ops = parse_edits(script)?;
    let mut summary = Vec::new();
    for op in ops {
        match op {
            EditOp::SetData {
                selector,
                data,
            } => {
                let idx = selector.find_index(bp).ok_or_else(|| {
                    format!("no node matching kind '{}' (nth {})", selector.kind, selector.nth)
                })?;
                bp.nodes[idx].data = data;
                summary.push(format!("{}#{} data", selector.kind, selector.nth));
            }
            EditOp::SetPin {
                selector,
                key,
                value,
            } => {
                let idx = selector.find_index(bp).ok_or_else(|| {
                    format!("no node matching kind '{}' (nth {})", selector.kind, selector.nth)
                })?;
                if !bp.nodes[idx].data.is_object() {
                    bp.nodes[idx].data = serde_json::json!({});
                }
                bp.nodes[idx].data.as_object_mut().map(|m| m.insert(key.clone(), value));
                summary.push(format!("{}#{} {key}", selector.kind, selector.nth));
            }
        }
    }
    Ok(summary.join(", "))
}

/// Opens an approval request on the shared broker and emits the matching
/// `approval_request` event. Returns whether the user allowed it.
///
/// Missing, cancelled or expired approval fails without becoming a user denial.
pub async fn await_approval(
    ctx: &mut ExecutionContext,
    request_type: &str,
    message: &str,
    detail: serde_json::Value,
) -> DaemonResult<bool> {
    let detail_json = serde_json::to_string(&detail)
        .unwrap_or_else(|_| serde_json::json!({ "request_type": request_type }).to_string());
    let subject = format!("{message}\n{detail_json}");
    let command_hash = crate::sandbox::command_hash(&subject);
    let broker = ctx
        .approvals
        .as_ref()
        .ok_or_else(|| DaemonError::Sandbox("no approval broker available".into()))?;
    if broker.is_closed() || ctx.cancel_requested.load(std::sync::atomic::Ordering::SeqCst) {
        return Err(DaemonError::Sandbox("approval run closed or cancelled".into()));
    }
    let grants =
        crate::sandbox::grant::GrantStore::new(ctx.workspace_db.clone(), ctx.global_db.clone());
    let persistent = grants.lookup(command_hash);
    if persistent == Some(false) {
        return Ok(false);
    }
    if let Some(allow) = broker.run_grant(command_hash).or(persistent) {
        return Ok(allow);
    }
    let timeout_secs = ctx
        .config
        .as_ref()
        .map(|c| c.try_read().map(|cfg| cfg.sandbox.approval_timeout_secs).unwrap_or(0))
        .unwrap_or(0)
        .max(30);
    let response =
        crate::sandbox::request_user_approval(ctx, command_hash, &subject, detail, timeout_secs)
            .await?;
    Ok(response.decision == Decision::Allow)
}

pub mod application;
pub(crate) mod draft;

/// Approve an exact proposal; the interpreter commits it at a safe boundary.
pub async fn approve_and_apply(
    ctx: &mut ExecutionContext,
    summary: &str,
    script: &serde_json::Value,
) -> DaemonResult<String> {
    application::approve(ctx, summary, script, application::Source::ModelTool).await
}

const PLAN_AGENT_SYSTEM: &str = "You are the Metteur plan agent. The task has \
    repeatedly failed its deterministic validation. Produce a JSON edit script \
    that fixes the Blueprint, addressing ONLY the failure described. Each edit \
    is one of: \
    {\"op\":\"set_data\",\"match\":{\"kind\":\"<Kind>\",\"nth\":1},\"data\":{...}} \
    or \
    {\"op\":\"set_pin\",\"match\":{\"kind\":\"<Kind>\",\"nth\":1},\"pin\":\"<pin>\",\"value\":<value>}. \
    Node kinds known: the failing validator/judge and the node kind listed in the task. \
    Output ONLY the JSON array, no prose.";

/// Runs the internal PlanAgent to produce an edit script for the failure.
///
/// `mock_text` lets tests script the provider output.
pub async fn run_plan_agent(
    ctx: &mut ExecutionContext,
    task: &str,
    mock_text: Option<String>,
) -> DaemonResult<serde_json::Value> {
    let mut system = crate::harness::fragments(ctx).await;
    system.push(metteur_shared::llm::SystemFragment {
        priority: crate::harness::sections::PRIORITY_NODE,
        scope: "plan_agent".to_string(),
        content: PLAN_AGENT_SYSTEM.to_string(),
    });
    let context = ContextManager::new_from_prompt(system, task);
    let opts = ReactOptions {
        provider: if mock_text.is_some() {
            "mock".to_string()
        } else {
            "openai-chat".to_string()
        },
        model: None,
        mock_text,
        allowed_tools: Some(std::collections::HashSet::new()),
        label: "PlanAgent".to_string(),
        ..Default::default()
    };
    let outcome = run_react(ctx, context, &opts).await?;
    let text = strip_code_fence(&outcome.text);
    serde_json::from_str(&text)
        .map_err(|e| DaemonError::Execution(format!("plan agent produced invalid JSON: {e}")))
}

/// Strips a ```json ... ``` fence an LLM may add around its answer.
fn strip_code_fence(text: &str) -> String {
    let trimmed = text.trim();
    let start = trimmed.find('[').unwrap_or(0);
    let end = trimmed.rfind(']').map(|i| i + 1).unwrap_or(trimmed.len());
    trimmed[start..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(kind: &str) -> metteur_shared::Node {
        metteur_shared::Node {
            id: uuid::Uuid::new_v4(),
            node_type: metteur_shared::NodeType::Function,
            kind: kind.to_string(),
            position: (0.0, 0.0),
            pins: Vec::new(),
            data: serde_json::json!({ "old": 1 }),
        }
    }

    fn bp() -> Blueprint {
        Blueprint {
            id: uuid::Uuid::new_v4(),
            name: "t".to_string(),
            nodes: vec![node("Validator"), node("CallLLM")],
            edges: Vec::new(),
            entry_node_id: uuid::Uuid::new_v4(),
        }
    }

    #[test]
    fn set_data_replaces_node_data() {
        let mut b = bp();
        let s = apply_edits(
            &mut b,
            &serde_json::json!([{
                "op": "set_data",
                "match": { "kind": "Validator", "nth": 1 },
                "data": { "mode": "not_empty" }
            }]),
        )
        .unwrap();
        assert!(s.contains("Validator#1"));
        assert_eq!(b.nodes[0].data["mode"], "not_empty");
    }

    #[test]
    fn set_pin_updates_one_key() {
        let mut b = bp();
        apply_edits(
            &mut b,
            &serde_json::json!([{
                "op": "set_pin",
                "match": { "kind": "CallLLM", "nth": 1 },
                "pin": "temperature",
                "value": 0.2
            }]),
        )
        .unwrap();
        assert_eq!(b.nodes[1].data["temperature"], 0.2);
        assert_eq!(b.nodes[1].data["old"], 1);
    }

    #[test]
    fn unknown_op_or_missing_match_is_rejected() {
        let mut b = bp();
        assert!(apply_edits(&mut b, &serde_json::json!([{ "op": "nope" }])).is_err());
        assert!(apply_edits(
            &mut b,
            &serde_json::json!([{ "op": "set_data", "match": { "kind": "Validator", "nth": 9 }, "data": {} }])
        )
        .is_err());
    }

    #[test]
    fn strips_code_fence() {
        assert_eq!(strip_code_fence("```json\n[{\"op\":\"x\"}]\n```"), "[{\"op\":\"x\"}]");
    }
}
