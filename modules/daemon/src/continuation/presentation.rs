//! Read-only workflow facts. Actions derive from host state, never model prose.
use super::{
    state::{DispatchState, TaskState, Terminal},
    store::TaskRecord,
};
use serde_json::{Value, json};

pub fn workflow(r: &TaskRecord, current_revision: u64, may_send: bool, now: u64) -> Value {
    workflow_with_approval(r, current_revision, may_send, now, false)
}
pub fn workflow_with_approval(
    r: &TaskRecord,
    current_revision: u64,
    may_send: bool,
    now: u64,
    approval_pending: bool,
) -> Value {
    let t = &r.task;
    let effects = t.sources.values().any(|s| {
        s.may_write
            && matches!(
                s.terminal,
                Some(Terminal::Indeterminate | Terminal::Interrupted | Terminal::Cancelled)
            )
    }) || t.dispatch.as_ref().is_some_and(|d| {
        matches!(d.state, DispatchState::InDoubt)
            || (t.state == TaskState::NeedsAttention && d.state == DispatchState::Started)
    });
    let (code, reason, next) = if t.state == TaskState::Stopped {
        (
            "stopped",
            "Stopped by the user",
            "Review the retained result. Starting again needs a new plan.",
        )
    } else if effects {
        (
            "effects_unknown",
            "Background effects need review",
            "Inspect the journal, files and receipts before any new execution.",
        )
    } else if t.state == TaskState::Completed {
        ("completed", "Task finished", "Review the recorded checks and result.")
    } else if t.budget.unknown_usage {
        (
            "usage_unknown",
            "Usage is unconfirmed",
            "Review recorded usage. The remaining budget has not been reset.",
        )
    } else if !may_send {
        (
            "permission_revoked",
            "Current permission does not allow continuation",
            "Review permissions, then prepare a new explicit plan.",
        )
    } else if t.budget.used_turns >= t.budget.max_turns
        || t.budget.consumed_tokens.saturating_add(t.budget.reserved_tokens) >= t.budget.token_limit
    {
        (
            "budget_exhausted",
            "Continuation limit reached",
            "Review usage and prepare a new plan with explicit limits.",
        )
    } else if r.policy.as_ref().is_none_or(|p| p.expires_ms <= now) {
        (
            "policy_expired",
            "Continuation authorization expired",
            "Prepare a new plan with current scope and expiry.",
        )
    } else if current_revision != t.expected_revision {
        (
            "conversation_changed",
            "Newer conversation input superseded this wait",
            "Add the old result to a draft or prepare a new binding.",
        )
    } else if t.state == TaskState::NeedsAttention {
        (
            "recovery_required",
            "Task needs review",
            "Inspect its recorded reason and result before preparing a new plan.",
        )
    } else if t.paused || t.state == TaskState::Paused {
        (
            "paused",
            "Automatic continuation is paused",
            "Resume the existing policy or keep reviewing.",
        )
    } else if approval_pending {
        (
            "approval_required",
            "Approval is required",
            "Review the exact requested action. Existing limits and permissions still apply.",
        )
    } else if t.wait.is_some() && !matches!(t.state, TaskState::Active | TaskState::Dispatching) {
        (
            "waiting",
            "Waiting for recorded sources",
            "Review sources or pause automatic continuation.",
        )
    } else {
        ("active", "Task is active", "Review current execution or stop the task.")
    };
    let mut actions = vec!["view_results", "add_to_draft"];
    if !matches!(t.state, TaskState::Stopped | TaskState::Completed) {
        actions.push("stop");
    }
    if code == "paused" {
        actions.push("resume");
    }
    if matches!(code, "active" | "waiting") {
        actions.push("pause");
    }
    if !matches!(code, "effects_unknown" | "usage_unknown") {
        actions.push("new_plan");
    }
    if code == "permission_revoked" {
        actions.push("open_settings");
    }
    if super::rebind::eligible(r, current_revision, may_send, now) {
        actions.push("rebind");
    }
    let waiting: Vec<_> = t
        .wait
        .iter()
        .flat_map(|w| w.sources.iter())
        .filter_map(|id| t.sources.get(id))
        .map(|s| json!({"kind":s.kind,"terminal":s.terminal}))
        .collect();
    json!({"goal":r.policy.as_ref().map(|p|p.purpose.as_str()).unwrap_or("Recorded task"),"reason_code":code,"reason":reason,"next_action":next,"allowed_actions":actions,"waiting_on":waiting,"target":{"task":t.id,"session":t.session,"revision":r.revision},"origin":"host_state"})
}
