//! Explicitly bind an existing wait to newer input without replacing its ledger.
use super::{
    state::{TaskState, Terminal},
    store::TaskRecord,
};
use crate::{
    chat::session::ChatSessionRecord,
    error::{DaemonError, DaemonResult},
};
use serde_json::{Value, json};

pub fn eligible(r: &TaskRecord, current: u64, may_send: bool, now: u64) -> bool {
    r.task.expected_revision != current
        && may_send
        && r.task.policy_active
        && r.task.stop_generation.is_none()
        && r.policy.as_ref().is_some_and(|p| p.expires_ms > now)
        && !r.task.budget.unknown_usage
        && r.task.budget.used_turns < r.task.budget.max_turns
        && r.task.budget.reserved_tokens == 0
        && r.task.budget.consumed_tokens < r.task.budget.token_limit
        && matches!(
            r.task.state,
            TaskState::Waiting | TaskState::Ready | TaskState::Paused | TaskState::NeedsAttention
        )
        && r.task.dispatch.is_none()
        && r.saved_context.is_some()
        && r.task.wait.as_ref().is_some_and(|w| {
            !w.sources.is_empty()
                && w.sources.iter().all(|id| {
                    r.task
                        .sources
                        .get(id)
                        .is_some_and(|s| s.task == r.task.id && s.generation == r.task.generation)
                })
        })
        && !r.task.sources.values().any(|s| {
            s.may_write
                && (s.terminal.is_none()
                    || matches!(
                        s.terminal,
                        Some(Terminal::Indeterminate | Terminal::Interrupted | Terminal::Cancelled)
                    ))
        })
}

pub fn proposal(
    r: &TaskRecord,
    current: u64,
    context: &ChatSessionRecord,
    config_digest: &str,
    may_send: bool,
    now: u64,
) -> DaemonResult<Value> {
    if !eligible(r, current, may_send, now)
        || context.session_id != r.task.session
        || r.binding_digest.as_deref() != Some(config_digest)
    {
        return Err(DaemonError::Execution("Rebinding requires a superseded safe wait, current permission, unchanged model/tools, and remaining policy limits".into()));
    }
    let context_bytes =
        serde_json::to_vec(context).map_err(|e| DaemonError::Serialization(e.to_string()))?;
    let mut proposal = json!({
        "task":r.task.id,"revision":r.revision,"session":r.task.session,"session_revision":current,
        "goal":r.policy.as_ref().map(|p| &p.purpose),"policy":r.policy,"budget":r.task.budget,
        "wait":r.task.wait,"sources":r.task.sources,"config_digest":config_digest,
        "context_digest":crate::storage::resources::digest(&context_bytes),
        "latest_user_input":context.context.messages.iter().rev().find(|m| m.role == metteur_shared::llm::Role::User).map(|m| m.text_content().chars().take(2048).collect::<String>()),
        "origin":"host_state"
    });
    proposal["digest"] = crate::storage::resources::digest(proposal.to_string().as_bytes()).into();
    Ok(proposal)
}

pub fn apply(r: &mut TaskRecord, current: u64, context: ChatSessionRecord, now: u64) {
    // The generation, source/event identities, consumed bits and full budget
    // remain intact. Store::update persists the previous binding in history.
    r.task.expected_revision = current;
    r.saved_context = Some(context);
    r.task.attention = None;
    r.task.paused = false;
    r.task.state = TaskState::Waiting;
    r.task.reconcile(now);
}
