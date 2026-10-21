//! Command and file-write authorization: permission modes, whitelists and a
//! six-level approval flow.

pub mod approval;
pub mod approver;
pub mod grant;
pub mod mode;
pub mod policy;
pub mod predict;

use std::hash::Hasher;
use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::error::{DaemonError, DaemonResult};
use crate::execution::ExecutionEvent;
use crate::execution::context::ExecutionContext;

use approval::{ApprovalResponse, Decision};
use grant::GrantStore;
pub use mode::PermissionMode;
use predict::Risk as policy_risk;

/// Default seconds before an unanswered approval request is denied.
const DEFAULT_APPROVAL_TIMEOUT_SECS: u64 = 300;

/// Resolves the final allow/deny verdict for running `command`.
///
/// The check order is: persistent deny grants, run-scoped grants, policy
/// whitelist, persistent allow grants, then the interactive approval flow.
/// When no approval channel is attached (e.g. unit tests), non-whitelisted
/// commands fail closed.
pub async fn authorize(ctx: &ExecutionContext, command: &str) -> DaemonResult<bool> {
    if ctx.cancel_requested.load(Ordering::SeqCst)
        || ctx.approvals.as_ref().is_some_and(|broker| broker.is_closed())
    {
        return Ok(false);
    }
    let config = match &ctx.config {
        Some(config) => config.read().await.clone(),
        None => metteur_shared::config::Config::default(),
    };
    let sandbox_cfg = config.sandbox.clone();
    let mode = ctx.permission_mode;
    // `ask` means every command is confirmed, so the whitelist is not consulted.
    if !sandbox_cfg.enabled && mode != PermissionMode::Ask {
        return Ok(true);
    }

    let hash = command_hash(&policy::normalize(command));
    let grants = GrantStore::new(ctx.workspace_db.clone(), ctx.global_db.clone());

    // 1. A persisted deny overrides everything, including the whitelist.
    if grants.lookup(hash) == Some(false) {
        ctx.audit("sandbox.denied", serde_json::json!({ "command": command }));
        return Ok(false);
    }

    // 2. Run-scoped grants from earlier decisions in this execution.
    if let Some(allow) = ctx.approvals.as_ref().and_then(|b| b.run_grant(hash)) {
        if !allow {
            ctx.audit("sandbox.denied", serde_json::json!({ "command": command }));
        }
        return Ok(allow && !ctx.cancel_requested.load(Ordering::SeqCst));
    }

    // 3. Whitelisted commands run directly (except in `ask`, where the user
    //    asked to see everything).
    if mode != PermissionMode::Ask
        && policy::classify(command, &sandbox_cfg) == policy::PolicyVerdict::Allowed
    {
        return Ok(true);
    }

    // 4. A persisted allow covers unclassified commands.
    if grants.lookup(hash) == Some(true) {
        return Ok(true);
    }

    // 5. In `full` mode the risky decisions are delegated to the model: the
    //    predictor already says which those are. A reviewer that cannot answer
    //    falls through to the user rather than allowing anything, so this runs
    //    before the approval channel is required.
    let impact = predict::predict(command);
    let detail = serde_json::json!({
        "tool": "ExecuteCommand",
        "node_id": ctx.current_node.to_string(),
        "command": command,
        "impact": {
            "program": impact.program,
            "affected_paths": impact.affected_paths,
            "risk": impact.risk.as_str(),
        },
    });
    if mode == PermissionMode::Full
        && impact.risk != policy_risk::Low
        && let Some(allow) = approver::review(
            ctx,
            &approver::ReviewRequest {
                kind: "command",
                subject: command,
                detail: &detail,
            },
        )
        .await
    {
        if let Some(broker) = &ctx.approvals {
            broker.record_run_grant(hash, allow);
        }
        ctx.audit(
            "sandbox.review",
            serde_json::json!({ "command": command, "outcome": if allow { "allowed" } else { "denied" } }),
        );
        return Ok(allow
            && !ctx.cancel_requested.load(Ordering::SeqCst)
            && !ctx.approvals.as_ref().is_some_and(|broker| broker.is_closed()));
    }

    // 6. Otherwise the user answers, through the channel attached to this run.
    let response =
        match request_user_approval(ctx, hash, command, detail, approval_timeout(&config.sandbox))
            .await
        {
            Ok(response) => response,
            Err(_) => return Ok(false),
        };
    let allow = response.decision == Decision::Allow;
    if let Some(metrics) = &ctx.metrics {
        use std::sync::atomic::Ordering;
        if allow {
            metrics.sandbox_approvals_allowed.fetch_add(1, Ordering::Relaxed);
        } else {
            metrics.sandbox_approvals_denied.fetch_add(1, Ordering::Relaxed);
        }
    }
    Ok(allow)
}

/// Resolves whether a file mutation may proceed.
///
/// `path` is the absolute target, `subject` the workspace-relative path shown to
/// whoever decides, and `summary` a short description of the change (an edit
/// summary, a file size). The same three modes apply as for commands:
///
/// - `ask`: the user confirms every write;
/// - `sandbox`: writes inside the workspace run, writes outside are confirmed;
/// - `full`: writes outside the workspace are reviewed by the model, then by the
///   user if it cannot answer.
///
/// A required confirmation fails closed when its broker is unavailable.
pub async fn authorize_write(
    ctx: &ExecutionContext,
    path: &std::path::Path,
    subject: &str,
    summary: &str,
) -> DaemonResult<bool> {
    if let (Some(graph), Some(db), Some(versions)) =
        (&ctx.blueprint, &ctx.workspace_db, &ctx.version_manager)
    {
        let id = graph.read().id;
        if let Some(bound) = crate::storage::blueprint_files::binding(db, id)?
            && versions.blueprint_path(&bound.blueprint_uri)? == path
        {
            return Err(DaemonError::PermissionDenied(
                "use ReplanBlueprint to modify the executing blueprint".into(),
            ));
        }
    }
    if ctx.cancel_requested.load(Ordering::SeqCst)
        || ctx.approvals.as_ref().is_some_and(|broker| broker.is_closed())
    {
        return Ok(false);
    }
    let config = match &ctx.config {
        Some(config) => config.read().await.clone(),
        None => metteur_shared::config::Config::default(),
    };
    let mode = ctx.permission_mode;
    let inside_workspace = path.starts_with(&ctx.workspace_root);
    let detail = serde_json::json!({
        "tool": "WriteFile",
        "node_id": ctx.current_node.to_string(),
        "path": subject,
        "summary": summary,
        "outside_workspace": !inside_workspace,
    });

    // Preserve the existing normalized-subject key and database boundaries.
    let hash = command_hash(&policy::normalize(subject));
    let grants = GrantStore::new(ctx.workspace_db.clone(), ctx.global_db.clone());
    if grants.lookup(hash) == Some(false) {
        return Ok(false);
    }
    if let Some(allow) = ctx.approvals.as_ref().and_then(|broker| broker.run_grant(hash)) {
        return Ok(allow);
    }
    if grants.lookup(hash) == Some(true) {
        return Ok(true);
    }
    if mode == PermissionMode::Sandbox && inside_workspace {
        return Ok(true);
    }
    if mode == PermissionMode::Full && inside_workspace {
        return Ok(true);
    }
    if mode == PermissionMode::Full
        && let Some(allow) = approver::review(
            ctx,
            &approver::ReviewRequest {
                kind: "file",
                subject,
                detail: &detail,
            },
        )
        .await
    {
        if !allow {
            ctx.audit(
                "sandbox.review",
                serde_json::json!({ "path": subject, "outcome": "denied" }),
            );
        }
        return Ok(allow
            && !ctx.cancel_requested.load(Ordering::SeqCst)
            && !ctx.approvals.as_ref().is_some_and(|broker| broker.is_closed()));
    }

    Ok(request_user_approval(ctx, hash, subject, detail, approval_timeout(&config.sandbox))
        .await
        .is_ok_and(|response| response.decision == Decision::Allow))
}

/// Emits and awaits one user request without losing its scope or failure cause.
pub(crate) async fn request_user_approval(
    ctx: &ExecutionContext,
    hash: u64,
    subject: &str,
    detail: serde_json::Value,
    timeout_secs: u64,
) -> DaemonResult<ApprovalResponse> {
    let result = async {
        let broker = ctx.approvals.as_ref().ok_or_else(|| {
            DaemonError::Sandbox("no approval broker available".into())
        })?;
        let request = broker.open_request(
            hash, subject, Duration::from_secs(timeout_secs), ctx.cancel_requested.clone(),
        ).map_err(|error| DaemonError::Sandbox(error.to_string()))?;
        let request_id = request.id().to_string();
        if let Some(events) = &ctx.events {
            events.send(ExecutionEvent::ApprovalRequested {
                node_id: ctx.current_node,
                request_id: request_id.clone(),
                detail: detail.to_string(),
            }).map_err(|_| DaemonError::Sandbox("approval event channel closed".into()))?;
        }
        ctx.audit("sandbox.approval", serde_json::json!({
            "subject": subject, "request_id": request_id, "outcome": "pending",
        }));
        let _human_wait = ctx.approval_wait_clock.as_ref().map(|clock| clock.enter());
        match request.wait().await {
            Ok(response) => {
                ctx.audit("sandbox.approval", serde_json::json!({
                    "subject": subject, "request_id": response.request_id,
                    "outcome": if response.decision == Decision::Allow { "allowed" } else { "denied" },
                    "scope": format!("{:?}", response.scope),
                }));
                Ok(response)
            }
            Err(error) => {
                ctx.audit("sandbox.approval", serde_json::json!({
                    "subject": subject, "request_id": request_id,
                    "outcome": "unavailable", "reason": error.to_string(),
                }));
                Err(DaemonError::Sandbox(error.to_string()))
            }
        }
    }.await;
    if let Err(error) = &result {
        ctx.audit(
            "sandbox.approval_failed",
            serde_json::json!({
                "subject": subject, "reason": error.to_string(),
            }),
        );
    }
    result
}

/// Seconds to wait for an approval before denying it.
///
/// One setting covers commands and file writes: a user who tunes the timeout
/// means "how long do I have to answer", not "for which kind of operation".
fn approval_timeout(config: &metteur_shared::config::SandboxConfig) -> u64 {
    if config.approval_timeout_secs > 0 {
        config.approval_timeout_secs
    } else {
        DEFAULT_APPROVAL_TIMEOUT_SECS
    }
}

/// Returns the stable grant key for a normalized command line.
pub fn command_hash(normalized_command: &str) -> u64 {
    let mut hasher = twox_hash::XxHash64::default();
    hasher.write(normalized_command.as_bytes());
    hasher.finish()
}

/// Collapses duplicated lines and truncates oversized tool output.
///
/// Consecutive duplicate lines are folded into a single marker; output longer
/// than `max_bytes` keeps its head and tail halves joined by an omission
/// marker.
pub fn compress_output(output: &str, max_bytes: usize) -> String {
    let deduped = fold_duplicate_lines(output);
    if deduped.len() <= max_bytes {
        return deduped;
    }
    let half = max_bytes / 2;
    let head: String = truncate_chars(&deduped, half, false);
    let tail: String = truncate_chars(&deduped, half, true);
    format!("{head}\n... {0} bytes omitted ...\n{tail}", deduped.len() - head.len() - tail.len())
}

fn fold_duplicate_lines(output: &str) -> String {
    let mut out = String::new();
    let mut lines = output.lines().peekable();
    while let Some(line) = lines.next() {
        let mut repeats = 1usize;
        while lines.peek() == Some(&line) && repeats < usize::MAX {
            lines.next();
            repeats += 1;
        }
        out.push_str(line);
        out.push('\n');
        if repeats > 1 {
            out.push_str(&format!("... {} duplicate lines omitted ...\n", repeats - 1));
        }
    }
    if output.ends_with('\n') || out.is_empty() {
        return out;
    }
    out.strip_suffix('\n').map(str::to_string).unwrap_or(out)
}

fn truncate_chars(text: &str, max_bytes: usize, from_end: bool) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut taken = 0usize;
    if from_end {
        let collected: String = text
            .chars()
            .rev()
            .take_while(|c| {
                let size = c.len_utf8();
                let keep = taken < max_bytes;
                taken += size;
                keep
            })
            .collect();
        collected.chars().rev().collect()
    } else {
        text.chars()
            .take_while(|c| {
                let size = c.len_utf8();
                let keep = taken < max_bytes;
                taken += size;
                keep
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn command_hash_is_stable_and_normalized_input_only() {
        assert_eq!(command_hash("cargo build"), command_hash("cargo build"));
        assert_ne!(command_hash("cargo build"), command_hash("cargo test"));
    }

    #[test]
    fn folds_consecutive_duplicate_lines() {
        let input = "a\na\na\nb\n";
        let out = compress_output(input, usize::MAX);
        assert_eq!(out, "a\n... 2 duplicate lines omitted ...\nb\n");
    }

    #[test]
    fn truncates_oversized_output_head_and_tail() {
        let long_line = "x".repeat(100_000);
        let out = compress_output(&long_line, 8192);
        assert!(out.len() < 10_000);
        assert!(out.contains("bytes omitted"));
        assert!(out.starts_with('x'));
        assert!(out.ends_with('x'));
    }

    #[tokio::test]
    async fn disabled_sandbox_allows_everything() {
        let mut ctx = test_context();
        ctx.config =
            Some(Arc::new(tokio::sync::RwLock::new(metteur_shared::config::Config::default())));
        assert!(authorize(&ctx, "rm -rf /").await.unwrap());
    }

    #[tokio::test]
    async fn whitelisted_command_runs_without_broker() {
        let mut cfg = metteur_shared::config::Config::default();
        cfg.sandbox.enabled = true;
        cfg.sandbox.whitelist = vec!["echo *".to_string()];
        let mut ctx = test_context();
        ctx.config = Some(Arc::new(tokio::sync::RwLock::new(cfg)));
        assert!(authorize(&ctx, "echo hi").await.unwrap());
    }

    #[tokio::test]
    async fn unlisted_command_fails_closed_without_broker() {
        let mut cfg = metteur_shared::config::Config::default();
        cfg.sandbox.enabled = true;
        let mut ctx = test_context();
        ctx.config = Some(Arc::new(tokio::sync::RwLock::new(cfg)));
        assert!(!authorize(&ctx, "python train.py").await.unwrap());
    }

    fn test_context() -> ExecutionContext {
        ExecutionContext::new(
            Arc::new(crate::registry::Registry::with_builtins()),
            crate::llm::LlmClientFactory::new(),
            std::env::temp_dir(),
        )
    }
}
