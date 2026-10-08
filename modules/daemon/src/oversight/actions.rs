//! Trusted proposal state. Neither identity, lineage nor decisions are model input.
use super::{
    diagnostic::{self, Category, Diagnostic, Stage},
    requests::{self, State},
    scheduler::{self, Status},
};
use crate::{
    DaemonError, DaemonResult,
    execution::context::ExecutionContext,
    storage::persistence::{Db, cf},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proposal {
    pub proposal_id: Uuid,
    pub review_id: Uuid,
    pub run_id: Uuid,
    pub source_request_ids: Vec<Uuid>,
    pub kind: String,
    pub state: State,
    pub binding: Value,
    pub result_refs: Vec<String>,
    pub reason: String,
    #[serde(default)]
    pub decision_source: String,
    #[serde(default)]
    pub diagnostic: Option<Diagnostic>,
}
fn error(text: &str) -> DaemonError {
    DaemonError::Execution(text.into())
}
fn encode(value: &impl Serialize) -> DaemonResult<Vec<u8>> {
    serde_json::to_vec(value).map_err(|e| DaemonError::Serialization(e.to_string()))
}
fn save(db: &Db, run: Uuid, s: &scheduler::Schedule, q: &requests::Queue) -> DaemonResult<()> {
    db.put_pair_durable(
        cf::EXECUTION_STATE,
        format!("oversight:schedule:{run}").as_bytes(),
        &encode(s)?,
        format!("oversight:requests:{run}").as_bytes(),
        &encode(q)?,
    )?;
    db.oversight_notify.notify_one();
    Ok(())
}
pub(crate) fn enabled(ctx: &ExecutionContext) -> DaemonResult<()> {
    let config = ctx
        .config
        .as_ref()
        .ok_or_else(|| error("Oversight actions require configuration"))?
        .try_read()
        .map_err(|_| error("Configuration is changing"))?;
    let settings = metteur_shared::config::oversight::OversightConfig::from_config(&config)
        .map_err(|_| error("Invalid oversight settings"))?;
    if settings.mode == "off" {
        return Err(error("Oversight actions are disabled in off mode"));
    }
    if ctx.cancel_requested.load(std::sync::atomic::Ordering::SeqCst)
        || ctx.approvals.as_ref().is_none_or(|b| b.is_closed())
    {
        return Err(error("Run is closing"));
    }
    Ok(())
}
pub(crate) fn register(
    ctx: &ExecutionContext,
    review: Uuid,
    id: Uuid,
    kind: &str,
    binding: Value,
) -> DaemonResult<Guard> {
    enabled(ctx)?;
    let db = ctx.workspace_db.as_ref().ok_or_else(|| error("Workspace database unavailable"))?;
    let run = ctx.run_id;
    let _gate = db.oversight_gate.lock().map_err(|_| error("Oversight lock poisoned"))?;
    let mut s = scheduler::load(db, run)?.ok_or_else(|| error("Review unavailable"))?;
    if s.closed {
        return Err(error("Run is closed"));
    }
    let r = s
        .reviews
        .iter_mut()
        .find(|r| r.review_id == review && r.status == Status::Running)
        .ok_or_else(|| error("Review is not running"))?;
    // One concrete action per review keeps confirmation and results unambiguous.
    if !r.proposals.is_empty() {
        return Err(error("This review already proposed an action"));
    }
    let mut q = requests::load(db, run)?;
    if q.closed {
        return Err(error("Requests closed"));
    }
    for id in &r.source_request_ids {
        if !q.requests.iter().any(|q| {
            q.request_id == *id
                && q.review_id == Some(review)
                && q.state == State::Reviewing
                && q.source == "concierge_forwarded"
        }) {
            return Err(error("Request lineage changed"));
        }
    }
    let p = Proposal {
        proposal_id: id,
        review_id: review,
        run_id: run,
        source_request_ids: r.source_request_ids.iter().copied().collect(),
        kind: kind.into(),
        state: State::AwaitingConfirmation,
        binding,
        result_refs: vec![],
        reason: String::new(),
        decision_source: String::new(),
        diagnostic: None,
    };
    for request in &mut q.requests {
        if r.source_request_ids.contains(&request.request_id) {
            request.state = State::AwaitingConfirmation;
            request.revision += 1;
            request.proposals.push(requests::ProposalResult {
                proposal_id: id,
                state: State::AwaitingConfirmation,
                result_refs: vec![],
            });
        }
    }
    r.proposals.push(p);
    save(db, run, &s, &q)?;
    Ok(Guard {
        db: db.clone(),
        run,
        id,
        armed: true,
    })
}
pub(crate) fn transition(
    db: &Db,
    run: Uuid,
    id: Uuid,
    state: State,
    refs: Vec<String>,
    reason: &str,
) -> DaemonResult<()> {
    transition_with(
        db,
        run,
        id,
        Change {
            state,
            refs,
            reason,
            completes_review: false,
            diagnostic: None,
        },
        || Ok(()),
    )
}
struct Change<'a> {
    state: State,
    refs: Vec<String>,
    reason: &'a str,
    completes_review: bool,
    diagnostic: Option<Diagnostic>,
}
fn transition_with(
    db: &Db,
    run: Uuid,
    id: Uuid,
    change: Change<'_>,
    apply: impl FnOnce() -> DaemonResult<()>,
) -> DaemonResult<()> {
    let Change {
        state,
        refs,
        reason,
        completes_review,
        diagnostic,
    } = change;
    let _gate = db.oversight_gate.lock().map_err(|_| error("Oversight lock poisoned"))?;
    let mut s = scheduler::load(db, run)?.ok_or_else(|| error("Review unavailable"))?;
    if s.closed {
        return Err(error("Run is closed"));
    }
    let r = s
        .reviews
        .iter_mut()
        .find(|r| r.proposals.iter().any(|p| p.proposal_id == id))
        .ok_or_else(|| error("Proposal unavailable"))?;
    let p = r.proposals.iter_mut().find(|p| p.proposal_id == id).expect("selected proposal");
    let valid = matches!(
        (p.state, state),
        (
            State::AwaitingConfirmation,
            State::ApprovedPendingApply | State::Rejected | State::Failed
        ) | (State::ApprovedPendingApply, State::Applied | State::Failed)
    );
    if !valid || (state == State::Applied && refs.is_empty()) {
        return Err(error("Stale or invalid proposal transition"));
    }
    let mut q = requests::load(db, run)?;
    if q.closed {
        return Err(error("Requests closed"));
    }
    if completes_review && !matches!(p.kind.as_str(), "PauseRun" | "CancelRun") {
        return Err(error("Not a control proposal"));
    }
    for source in &p.source_request_ids {
        if !q
            .requests
            .iter()
            .any(|r| r.request_id == *source && r.proposals.iter().any(|p| p.proposal_id == id))
        {
            return Err(error("Proposal lineage missing"));
        }
    }
    apply()?;
    p.state = state;
    p.result_refs = refs.clone();
    p.reason = reason.into();
    if let Some(mut detail) = diagnostic {
        detail.call_id = r.work.call_ids.last().copied();
        p.diagnostic = Some(detail);
    } else if state == State::Rejected {
        let mut detail = Diagnostic::new(Category::ApprovalRejected, Stage::Approval, run, r.review_id);
        detail.proposal_id = Some(id);
        detail.call_id = r.work.call_ids.last().copied();
        p.diagnostic = Some(detail);
    }
    for request in &mut q.requests {
        if p.source_request_ids.contains(&request.request_id) {
            let child = request
                .proposals
                .iter_mut()
                .find(|p| p.proposal_id == id)
                .ok_or_else(|| error("Proposal lineage missing"))?;
            child.state = state;
            child.result_refs = refs.clone();
            request.state = state;
            request.result_refs = refs.clone();
            request.revision += 1;
        }
    }
    if state == State::Applied {
        r.actual_action_refs.extend(refs);
        if r.status == Status::Completed {
            r.verdict = Some("action_taken".into());
        }
    }
    if completes_review {
        r.status = Status::Completed;
        r.finished_at = Some(scheduler::now());
        r.summary = reason.into();
        r.verdict = Some("action_taken".into());
    }
    save(db, run, &s, &q)
}
pub(crate) fn apply_control(
    db: &Db,
    run: Uuid,
    id: Uuid,
    kind: &str,
    apply: impl FnOnce() -> DaemonResult<()>,
) -> DaemonResult<()> {
    let reason = format!(
        "{kind} requested under the recorded decision source. Inspect the execution outcome for completion and rollback results."
    );
    transition_with(
        db,
        run,
        id,
        Change {
            state: State::Applied,
            refs: vec![format!("control:{kind}:{id}"), format!("run:{run}")],
            reason: &reason,
            completes_review: true,
            diagnostic: None,
        },
        apply,
    )
}
pub(crate) struct Guard {
    db: Db,
    run: Uuid,
    id: Uuid,
    armed: bool,
}
impl Guard {
    pub(crate) async fn confirm(&mut self, ctx: &ExecutionContext) -> DaemonResult<bool> {
        self.confirm_with_timeout(ctx, 120).await
    }
    pub(crate) async fn confirm_with_timeout(&mut self, ctx: &ExecutionContext, timeout_secs: u64) -> DaemonResult<bool> {
        enabled(ctx)?;
        let s = scheduler::load(&self.db, self.run)?.ok_or_else(|| error("Review unavailable"))?;
        let p = s
            .reviews
            .iter()
            .flat_map(|r| &r.proposals)
            .find(|p| p.proposal_id == self.id)
            .ok_or_else(|| error("Proposal unavailable"))?;
        let review = s
            .reviews
            .iter()
            .find(|r| r.review_id == p.review_id)
            .ok_or_else(|| error("Review unavailable"))?;
        if super::policy::delegated(ctx, review, p) {
            decision_source(&self.db, self.run, self.id, "delegated")?;
            transition(
                &self.db,
                self.run,
                self.id,
                State::ApprovedPendingApply,
                vec![],
                "Existing scoped delegation permits this system action; application is not complete",
            )?;
            return Ok(true);
        }
        let queue = requests::load(&self.db, self.run)?;
        let original:Vec<_>=queue.requests.iter().filter(|r|p.source_request_ids.contains(&r.request_id)).map(|r|json!({"request_id":r.request_id,"original_text":r.original_text,"concierge_note":r.concierge_note})).collect();
        let mut detail = p.binding.clone();
        detail["proposal_id"] = json!(p.proposal_id);
        detail["run_id"] = json!(p.run_id);
        detail["review_id"] = json!(p.review_id);
        detail["source_request_ids"] = json!(p.source_request_ids);
        detail["original_requests"] = json!(original);
        detail["confirmation_required"] = json!(true);
        let subject = format!("Confirm concrete oversight proposal {}", self.id);
        // Deliberately bypass all cached grants. Only this fresh broker request can approve.
        let response = crate::sandbox::request_user_approval(
            ctx,
            crate::sandbox::command_hash(&format!("{subject}\n{detail}")),
            &subject,
            detail,
            timeout_secs,
        )
        .await;
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                self.failed(&error, Stage::Approval)?;
                return Err(diagnostic::error(Category::ApprovalExpired, Stage::Approval));
            }
        };
        decision_source(&self.db, self.run, self.id, "human")?;
        if response.decision == crate::sandbox::approval::Decision::Deny {
            transition(
                &self.db,
                self.run,
                self.id,
                State::Rejected,
                vec![],
                "User rejected this proposal",
            )?;
            self.armed = false;
            return Ok(false);
        }
        if response.scope != crate::sandbox::approval::Scope::Once {
            return Err(error("Oversight proposals require a per-proposal AllowOnce confirmation"));
        }
        enabled(ctx)?;
        transition(
            &self.db,
            self.run,
            self.id,
            State::ApprovedPendingApply,
            vec![],
            "Confirmed; application is not complete",
        )?;
        Ok(true)
    }
    pub(crate) fn staged(&mut self) {
        self.armed = false;
    }
    pub(crate) fn failed(&mut self, error: &DaemonError, stage: Stage) -> DaemonResult<()> {
        fail(&self.db, self.run, self.id, error, stage)?;
        self.armed = false;
        Ok(())
    }
}
pub(crate) fn fail(db: &Db, run: Uuid, id: Uuid, error: &DaemonError, stage: Stage) -> DaemonResult<()> {
    let review = scheduler::load(db, run)?.and_then(|s| s.reviews.into_iter()
        .find(|r| r.proposals.iter().any(|p| p.proposal_id == id)))
        .ok_or_else(|| diagnostic::error(Category::Persistence, Stage::Persistence))?;
    let mut detail = Diagnostic::from_error(error, stage, run, review.review_id);
    detail.proposal_id = Some(id);
    let message = detail.message.clone();
    transition_with(db, run, id, Change { state: State::Failed, refs: vec![], reason: &message,
        completes_review: false, diagnostic: Some(detail) }, || Ok(()))
}
impl Drop for Guard {
    fn drop(&mut self) {
        if self.armed {
            let _ = fail(
                &self.db,
                self.run,
                self.id,
                &diagnostic::error(Category::ApprovalExpired, Stage::Approval),
                Stage::Approval,
            );
        }
    }
}

fn decision_source(db: &Db, run: Uuid, id: Uuid, source: &str) -> DaemonResult<()> {
    let _gate = db.oversight_gate.lock().map_err(|_| error("Oversight lock poisoned"))?;
    let mut s = scheduler::load(db, run)?.ok_or_else(|| error("Review unavailable"))?;
    if s.closed {
        return Err(error("Run closed"));
    }
    let p = s
        .reviews
        .iter_mut()
        .flat_map(|r| &mut r.proposals)
        .find(|p| p.proposal_id == id && p.state == State::AwaitingConfirmation)
        .ok_or_else(|| error("Proposal is no longer awaiting a decision"))?;
    p.decision_source = source.into();
    scheduler::save(db, run, &s)
}
pub(crate) fn authorize_application(ctx: &ExecutionContext, id: Uuid) -> DaemonResult<()> {
    enabled(ctx)?;
    let db = ctx.workspace_db.as_ref().ok_or_else(|| error("Workspace unavailable"))?;
    let s = scheduler::load(db, ctx.run_id)?.ok_or_else(|| error("Review unavailable"))?;
    if s.closed || requests::load(db, ctx.run_id)?.closed {
        return Err(error("Review closed"));
    }
    let r = s
        .reviews
        .iter()
        .find(|r| r.proposals.iter().any(|p| p.proposal_id == id))
        .ok_or_else(|| error("Proposal unavailable"))?;
    let p = r.proposals.iter().find(|p| p.proposal_id == id).expect("selected proposal");
    if p.state != State::ApprovedPendingApply || p.run_id != ctx.run_id {
        return Err(error("Proposal is not approved for this run"));
    }
    if p.decision_source == "delegated" && !super::policy::delegated(ctx, r, p) {
        return Err(error("Delegation no longer permits this action"));
    }
    Ok(())
}

pub(crate) fn reconcile_applied(
    db: &Db,
    run: Uuid,
    id: Uuid,
    refs: Vec<String>,
) -> DaemonResult<()> {
    let _gate = db.oversight_gate.lock().map_err(|_| error("Oversight lock poisoned"))?;
    let mut s = scheduler::load(db, run)?.ok_or_else(|| error("Applied review missing"))?;
    let mut q = requests::load(db, run)?;
    let r = s
        .reviews
        .iter_mut()
        .find(|r| r.proposals.iter().any(|p| p.proposal_id == id))
        .ok_or_else(|| error("Applied proposal missing"))?;
    let p = r.proposals.iter_mut().find(|p| p.proposal_id == id).expect("selected proposal");
    if p.run_id != run || p.kind != "blueprint_edits" {
        return Err(error("Applied proposal identity mismatch"));
    }
    if p.state == State::Applied && refs.iter().all(|reference| p.result_refs.contains(reference)) {
        return Ok(());
    }
    p.state = State::Applied;
    p.result_refs = refs.clone();
    p.reason =
        "Recovered a committed Version Flow and checkpoint acknowledgement; no action was replayed"
            .into();
    for reference in &refs {
        if !r.actual_action_refs.contains(reference) {
            r.actual_action_refs.push(reference.clone());
        }
    }
    if r.status == Status::Completed {
        r.verdict = Some("action_taken".into());
    }
    for source in &p.source_request_ids {
        let request = q
            .requests
            .iter_mut()
            .find(|request| request.request_id == *source)
            .ok_or_else(|| error("Applied request lineage missing"))?;
        let proposal = request
            .proposals
            .iter_mut()
            .find(|proposal| proposal.proposal_id == id)
            .ok_or_else(|| error("Applied request proposal missing"))?;
        proposal.state = State::Applied;
        proposal.result_refs = refs.clone();
        if request.proposals.iter().all(|p| p.state == State::Applied) {
            request.state = State::Applied;
        }
        for reference in &refs {
            if !request.result_refs.contains(reference) {
                request.result_refs.push(reference.clone());
            }
        }
        request.revision += 1;
    }
    save(db, run, &s, &q)
}

pub(crate) fn reconcile_unfinished(db: &Db, run: Uuid, id: Uuid, reason: &str) -> DaemonResult<()> {
    let _gate = db.oversight_gate.lock().map_err(|_| error("Oversight lock poisoned"))?;
    let mut s =
        scheduler::load(db, run)?.ok_or_else(|| error("Review unavailable for recovery"))?;
    let mut q = requests::load(db, run)?;
    let p = s
        .reviews
        .iter_mut()
        .flat_map(|r| &mut r.proposals)
        .find(|p| p.proposal_id == id && p.run_id == run)
        .ok_or_else(|| error("Proposal unavailable for recovery"))?;
    if p.state == State::Applied {
        return Err(error("Applied proposal contradicts recovery evidence"));
    }
    if p.state == State::ClosedUnhandled && p.reason == reason {
        return Ok(());
    }
    p.state = State::ClosedUnhandled;
    p.reason = reason.into();
    for source in &p.source_request_ids {
        let request = q
            .requests
            .iter_mut()
            .find(|r| r.request_id == *source)
            .ok_or_else(|| error("Recovery request lineage missing"))?;
        let proposal = request
            .proposals
            .iter_mut()
            .find(|p| p.proposal_id == id)
            .ok_or_else(|| error("Recovery proposal lineage missing"))?;
        proposal.state = State::ClosedUnhandled;
        request.state = State::ClosedUnhandled;
        request.revision += 1;
    }
    save(db, run, &s, &q)
}
