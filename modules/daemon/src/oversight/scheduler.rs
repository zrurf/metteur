//! Durable event coalescing. Only a trusted consumer can claim a review.
use super::requests;
use crate::{
    DaemonError, DaemonResult,
    execution::{ExecutionCheckpoint, RunStatus},
    storage::persistence::{Db, cf},
};
use metteur_shared::config::oversight::{NodeTrigger, OversightConfig};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pending,
    Running,
    Completed,
    Failed,
    TimedOut,
    BudgetExhausted,
    Cancelled,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Work {
    pub model: String,
    pub call_ids: Vec<Uuid>,
    pub notes: Vec<String>,
    pub answers: Vec<String>,
    pub evidence: Vec<Evidence>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    pub entry_id: String,
    pub node_id: Option<String>,
    pub scope: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Review {
    pub review_id: Uuid,
    pub run_id: Uuid,
    pub triggers: BTreeSet<String>,
    pub source_request_ids: BTreeSet<Uuid>,
    pub priority: bool,
    pub status: Status,
    pub created_at: u64,
    pub started_at: Option<u64>,
    pub finished_at: Option<u64>,
    pub summary: String,
    pub verdict: Option<String>,
    #[serde(default)]
    pub model_verdict: Option<String>,
    #[serde(default)]
    pub human_dispositions: Vec<serde_json::Value>,
    #[serde(default)]
    pub circuit_node: Option<Uuid>,
    pub notes: Vec<String>,
    pub actual_action_refs: Vec<String>,
    #[serde(default)]
    pub work: Work,
    #[serde(default)]
    pub proposals: Vec<super::actions::Proposal>,
    #[serde(default)]
    pub diagnostic: Option<super::diagnostic::Diagnostic>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Schedule {
    pub closed: bool,
    pub settings: OversightConfig,
    pub reviews: Vec<Review>,
    #[serde(default)]
    pub cancel_result: Option<super::control::CancelResult>,
    last_started: Option<u64>,
    finished_nodes: BTreeSet<u64>,
}
pub fn now() -> u64 {
    chrono::Utc::now().timestamp_millis().max(0) as u64
}
fn key(run: Uuid) -> String {
    format!("oversight:schedule:{run}")
}
fn error(message: &str) -> DaemonError {
    DaemonError::Execution(message.into())
}
fn encode(value: &impl Serialize) -> DaemonResult<Vec<u8>> {
    serde_json::to_vec(value).map_err(|e| DaemonError::Serialization(e.to_string()))
}
pub fn load(db: &Db, run: Uuid) -> DaemonResult<Option<Schedule>> {
    db.get(cf::EXECUTION_STATE, key(run).as_bytes())?
        .map(|v| serde_json::from_slice(&v).map_err(|e| DaemonError::Serialization(e.to_string())))
        .transpose()
}
pub(crate) fn save(db: &Db, run: Uuid, value: &Schedule) -> DaemonResult<()> {
    db.put_durable(cf::EXECUTION_STATE, key(run).as_bytes(), &encode(value)?)?;
    db.oversight_notify.notify_one();
    Ok(())
}
fn enqueue(
    s: &mut Schedule,
    run: Uuid,
    trigger: &str,
    ids: impl IntoIterator<Item = Uuid>,
    priority: bool,
    at: u64,
) {
    if s.closed {
        return;
    }
    let pending = s.reviews.iter().position(|r| r.status == Status::Pending).unwrap_or_else(|| {
        s.reviews.push(Review {
            review_id: Uuid::new_v4(),
            run_id: run,
            triggers: BTreeSet::new(),
            source_request_ids: BTreeSet::new(),
            priority: false,
            status: Status::Pending,
            created_at: at,
            started_at: None,
            finished_at: None,
            summary: String::new(),
            verdict: None,
            model_verdict: None,
            human_dispositions: vec![],
            circuit_node: None,
            notes: vec![],
            actual_action_refs: vec![],
            work: Work::default(),
            proposals: vec![],
            diagnostic: None,
        });
        s.reviews.len() - 1
    });
    let review = &mut s.reviews[pending];
    review.triggers.insert(trigger.into());
    review.source_request_ids.extend(ids);
    review.priority |= priority;
}
/// Initialization is explicit admission, never a side effect of a read RPC.
pub fn initialize(db: &Db, run: Uuid, settings: OversightConfig) -> DaemonResult<()> {
    let _gate = db.oversight_gate.lock().map_err(|_| error("oversight lock poisoned"))?;
    let mut s = load(db, run)?.unwrap_or_default();
    for r in &mut s.reviews {
        for proposal in &mut r.proposals {
            if matches!(
                proposal.state,
                requests::State::AwaitingConfirmation | requests::State::ApprovedPendingApply
            ) {
                proposal.state = requests::State::ClosedUnhandled;
                proposal.reason =
                    "Previous execution was interrupted; old confirmation cannot be replayed"
                        .into();
            }
        }
        if matches!(r.status, Status::Running | Status::Pending) {
            r.status = Status::Cancelled;
            r.finished_at = Some(now());
            r.summary = "Previous review was interrupted; no action is implied.".into();
        }
    }
    s.closed = false;
    s.settings = settings;
    let mut queue = requests::load(db, run)?;
    if queue.closed
        || crate::execution::DbCheckpointSink::load(db, run)?
            .is_none_or(|cp| cp.status != RunStatus::Running)
    {
        return Err(error("run is not active"));
    }
    for request in &mut queue.requests {
        if matches!(
            request.state,
            requests::State::AwaitingConfirmation | requests::State::ApprovedPendingApply
        ) {
            request.state = requests::State::ClosedUnhandled;
            request.revision += 1;
            for proposal in &mut request.proposals {
                if matches!(
                    proposal.state,
                    requests::State::AwaitingConfirmation | requests::State::ApprovedPendingApply
                ) {
                    proposal.state = requests::State::ClosedUnhandled;
                }
            }
        }
        if request.state == requests::State::Reviewing
            && s.reviews
                .iter()
                .any(|r| Some(r.review_id) == request.review_id && r.status == Status::Cancelled)
        {
            request.state = requests::State::Failed;
            request.revision += 1;
            request.result_refs =
                request.review_id.map(|id| vec![format!("review:{id}")]).unwrap_or_default();
        }
    }
    let ids: Vec<_> = queue
        .requests
        .iter()
        .filter(|r| r.state == requests::State::Received)
        .map(|r| r.request_id)
        .collect();
    if !ids.is_empty() {
        enqueue(&mut s, run, "request", ids, true, now());
    }
    db.put_pair_durable(
        cf::EXECUTION_STATE,
        key(run).as_bytes(),
        &encode(&s)?,
        format!("oversight:requests:{run}").as_bytes(),
        &encode(&queue)?,
    )?;
    db.oversight_notify.notify_one();
    Ok(())
}
/// Caller holds the database gate. Only received requests trigger new reviews.
pub(crate) fn queue_locked(db: &Db, run: Uuid, queue: &requests::Queue) -> DaemonResult<()> {
    let Some(mut s) = load(db, run)? else {
        return Ok(());
    };
    let ids: Vec<_> = queue
        .requests
        .iter()
        .filter(|r| r.state == requests::State::Received)
        .map(|r| r.request_id)
        .collect();
    if ids.is_empty() || s.closed {
        return Ok(());
    }
    enqueue(&mut s, run, "request", ids, true, now());
    save(db, run, &s)
}
/// Checkpoint facts are trusted; output text is never interpreted as a trigger.
pub(crate) fn checkpoint_locked(db: &Db, cp: &ExecutionCheckpoint) -> DaemonResult<()> {
    let Some(mut s) = load(db, cp.run_id)? else {
        return Ok(());
    };
    if cp.status != RunStatus::Running {
        return close_locked(db, cp.run_id);
    }
    if s.closed {
        return Ok(());
    }
    let mut changed = false;
    for invocation in &cp.view.invocations {
        if invocation.finished_at.is_none() || !s.finished_nodes.insert(invocation.sequence) {
            continue;
        }
        changed = true;
        // A finished node fenced by the interpreter is a circuit boundary.
        // Its dedicated trigger must win before a generic trigger can be claimed.
        if cp.in_flight == Some(invocation.node_id) && cp.circuit_failures > 0 {
            continue;
        }
        let kind = cp
            .view
            .graphs
            .get(&invocation.scope)
            .and_then(|g| g.nodes.iter().find(|n| n.id == invocation.node_id))
            .map(|n| n.kind.as_str())
            .unwrap_or("");
        let enabled = match &s.settings.triggers.on_node_finished {
            NodeTrigger::Enabled(on) => *on,
            NodeTrigger::Nodes(nodes) => {
                nodes.iter().any(|n| n == kind || n == &invocation.node_id.to_string())
            }
        };
        if enabled {
            enqueue(&mut s, cp.run_id, "node_finished", [], false, now());
        }
        if s.settings.triggers.on_validation_failed && invocation.check == Some(false) {
            enqueue(&mut s, cp.run_id, "validation_failed", [], false, now());
        }
    }
    // Circuit triggers belong to the interpreter, which binds the failed node.
    if changed {
        save(db, cp.run_id, &s)?;
    }
    Ok(())
}
/// Explicit trusted triggers. Circuit repair additionally requires a bound node.
pub fn trigger(db: &Db, run: Uuid, reason: &str, at: u64) -> DaemonResult<Uuid> {
    if !matches!(reason, "interval" | "checkpoint" | "circuit") {
        return Err(error("unknown oversight trigger"));
    }
    let _gate = db.oversight_gate.lock().map_err(|_| error("oversight lock poisoned"))?;
    let mut s = load(db, run)?.ok_or_else(|| error("review scheduler unavailable"))?;
    if s.closed {
        return Err(error("review scheduler closed"));
    }
    enqueue(&mut s, run, reason, [], reason != "interval", at);
    let id =
        s.reviews.iter().find(|r| r.status == Status::Pending).expect("enqueued review").review_id;
    save(db, run, &s)?;
    Ok(id)
}
pub fn due_at(schedule: &Schedule) -> Option<u64> {
    if schedule.closed || schedule.reviews.iter().any(|r| r.status == Status::Running) {
        return None;
    }
    let pending = schedule.reviews.iter().find(|r| r.status == Status::Pending)?;
    Some(if pending.priority {
        0
    } else {
        schedule
            .last_started
            .map(|at| at.saturating_add(schedule.settings.cooldown_ms))
            .unwrap_or(0)
    })
}
/// Claim and request transitions commit in one synced batch; lineage is server-owned.
pub fn claim(db: &Db, run: Uuid, at: u64) -> DaemonResult<Option<Review>> {
    let _gate = db.oversight_gate.lock().map_err(|_| error("oversight lock poisoned"))?;
    let mut s = load(db, run)?.ok_or_else(|| error("review scheduler unavailable"))?;
    if due_at(&s).is_none_or(|due| due > at) {
        return Ok(None);
    }
    let mut queue = requests::load(db, run)?;
    if queue.closed {
        return Ok(None);
    }
    let review =
        s.reviews.iter_mut().find(|r| r.status == Status::Pending).expect("due pending review");
    // Pending provenance is immutable even if a request was independently
    // settled before claim. Such a request must not become an autonomous grant.
    for request in &mut queue.requests {
        if review.source_request_ids.contains(&request.request_id)
            && request.state == requests::State::Received
        {
            request.state = requests::State::Reviewing;
            request.review_id = Some(review.review_id);
            request.revision += 1;
        }
    }
    review.status = Status::Running;
    review.started_at = Some(at);
    s.last_started = Some(at);
    let result = review.clone();
    db.put_pair_durable(
        cf::EXECUTION_STATE,
        key(run).as_bytes(),
        &encode(&s)?,
        format!("oversight:requests:{run}").as_bytes(),
        &encode(&queue)?,
    )?;
    Ok(Some(result))
}
/// Models never supply status, identity, provenance or actual action references.
pub struct Outcome {
    pub status: Status,
    pub summary: String,
    pub verdict: Option<String>,
    pub notes: Vec<String>,
}
pub fn finish(db: &Db, run: Uuid, id: Uuid, outcome: Outcome) -> DaemonResult<Review> {
    finish_with_diagnostic(db, run, id, outcome, None)
}
pub(crate) fn finish_with_diagnostic(
    db: &Db, run: Uuid, id: Uuid, outcome: Outcome,
    diagnostic: Option<super::diagnostic::Diagnostic>,
) -> DaemonResult<Review> {
    let Outcome {
        status,
        summary,
        verdict,
        notes,
    } = outcome;
    let verdict = verdict.as_deref();
    if !matches!(
        status,
        Status::Completed
            | Status::Failed
            | Status::TimedOut
            | Status::BudgetExhausted
            | Status::Cancelled
    ) || (status != Status::Completed && verdict.is_some())
        || verdict.is_some_and(|v| !matches!(v, "ok" | "concern"))
    {
        return Err(error("invalid review outcome"));
    }
    let _gate = db.oversight_gate.lock().map_err(|_| error("oversight lock poisoned"))?;
    let mut s = load(db, run)?.ok_or_else(|| error("review scheduler unavailable"))?;
    if s.closed {
        return Err(error("review scheduler closed"));
    }
    let r = s
        .reviews
        .iter_mut()
        .find(|r| r.review_id == id && r.status == Status::Running)
        .ok_or_else(|| error("stale review completion"))?;
    r.status = status;
    r.diagnostic = diagnostic;
    r.summary = summary;
    r.model_verdict = verdict.map(str::to_string);
    r.verdict = if r.status == Status::Completed && !r.actual_action_refs.is_empty() {
        Some("action_taken".into())
    } else {
        verdict.map(str::to_string)
    };
    r.notes = notes;
    r.finished_at = Some(now());
    let result = r.clone();
    let mut queue = requests::load(db, run)?;
    for request in &mut queue.requests {
        if request.review_id == Some(id) && request.state == requests::State::Reviewing {
            request.state = if result.status == Status::Completed {
                requests::State::Answered
            } else {
                requests::State::Failed
            };
            request.revision += 1;
            request.result_refs = vec![format!("review:{id}")];
        }
    }
    db.put_pair_durable(
        cf::EXECUTION_STATE,
        key(run).as_bytes(),
        &encode(&s)?,
        format!("oversight:requests:{run}").as_bytes(),
        &encode(&queue)?,
    )?;
    db.oversight_notify.notify_one();
    Ok(result)
}
pub fn record_work(db: &Db, run: Uuid, id: Uuid, work: &Work) -> DaemonResult<()> {
    let _gate = db.oversight_gate.lock().map_err(|_| error("oversight lock poisoned"))?;
    let mut s = load(db, run)?.ok_or_else(|| error("Review unavailable"))?;
    if s.closed {
        return Err(error("Review closed"));
    }
    let r = s
        .reviews
        .iter_mut()
        .find(|r| r.review_id == id && r.status == Status::Running)
        .ok_or_else(|| error("Review is not running"))?;
    r.work = work.clone();
    save(db, run, &s)
}
pub(crate) fn close_locked(db: &Db, run: Uuid) -> DaemonResult<()> {
    let Some(mut s) = load(db, run)? else {
        return Ok(());
    };
    if s.closed {
        return Ok(());
    }
    s.closed = true;
    for r in &mut s.reviews {
        for proposal in &mut r.proposals {
            if matches!(
                proposal.state,
                requests::State::AwaitingConfirmation | requests::State::ApprovedPendingApply
            ) {
                proposal.state = requests::State::ClosedUnhandled;
                proposal.reason = "Run closed before action completed".into();
            }
        }
        if matches!(r.status, Status::Pending | Status::Running) {
            r.status = Status::Cancelled;
            r.finished_at = Some(now());
            r.verdict = None;
            r.summary = "Run closed before review completion.".into();
        }
    }
    save(db, run, &s)
}
pub fn close(db: &Db, run: Uuid) -> DaemonResult<()> {
    let _gate = db.oversight_gate.lock().map_err(|_| error("oversight lock poisoned"))?;
    close_locked(db, run)
}

/// Only a fresh broker reply can record a gate disposition; the report is unchanged.
pub(crate) fn disposition(
    db: &Db,
    run: Uuid,
    id: Uuid,
    node: Uuid,
    continued: bool,
) -> DaemonResult<()> {
    let _gate = db.oversight_gate.lock().map_err(|_| error("Oversight lock poisoned"))?;
    let mut s = load(db, run)?.ok_or_else(|| error("Review unavailable"))?;
    if s.closed {
        return Err(error("Review closed"));
    }
    let r = s
        .reviews
        .iter_mut()
        .find(|r| r.review_id == id)
        .ok_or_else(|| error("Review unavailable"))?;
    r.human_dispositions.push(serde_json::json!({"node_id":node,"at_ms":now(),"action":if continued {"continue_without_validation"} else {"retry_review"}}));
    save(db, run, &s)
}

/// Only the interpreter may bind a concrete failed node to a circuit review.
pub(crate) fn circuit(db: &Db, run: Uuid, node: Uuid) -> DaemonResult<Uuid> {
    let _gate = db.oversight_gate.lock().map_err(|_| error("Oversight lock poisoned"))?;
    let cp = crate::execution::DbCheckpointSink::load(db, run)?
        .ok_or_else(|| error("Circuit checkpoint unavailable"))?;
    if cp.in_flight != Some(node) || cp.circuit_failures == 0 {
        return Err(error("Not a circuit boundary"));
    }
    let mut s = load(db, run)?.ok_or_else(|| error("Supervisor unavailable"))?;
    if s.closed {
        return Err(error("Supervisor closed"));
    }
    enqueue(&mut s, run, "circuit", [], true, now());
    let r = s.reviews.iter_mut().find(|r| r.status == Status::Pending).expect("enqueued");
    r.circuit_node = Some(node);
    let id = r.review_id;
    save(db, run, &s)?;
    Ok(id)
}
