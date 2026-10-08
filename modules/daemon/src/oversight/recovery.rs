//! Inactive-workspace reconciliation. Never launch calls or replay decisions.
use super::{
    requests::{self, State},
    scheduler::{self, Status},
};
use crate::{
    DaemonError, DaemonResult,
    execution::DbCheckpointSink,
    storage::persistence::{Db, cf},
};
use std::collections::BTreeSet;
use uuid::Uuid;
fn unfinished(state: State) -> bool {
    matches!(
        state,
        State::Received
            | State::Reviewing
            | State::AwaitingConfirmation
            | State::ApprovedPendingApply
    )
}
pub fn recover(db: &Db) -> DaemonResult<()> {
    let mut runs = BTreeSet::new();
    for (key, _) in db.scan(cf::EXECUTION_STATE)? {
        if let Some(suffix) = key
            .strip_prefix(b"oversight:schedule:")
            .or_else(|| key.strip_prefix(b"oversight:requests:"))
            .or_else(|| key.strip_prefix(b"oversight:closing:"))
        {
            let run = std::str::from_utf8(suffix)
                .ok()
                .and_then(|s| Uuid::parse_str(s).ok())
                .ok_or_else(|| DaemonError::Persistence("Invalid oversight run key".into()))?;
            runs.insert(run);
        }
    }
    for run in runs {
        let _gate = db
            .oversight_gate
            .lock()
            .map_err(|_| DaemonError::Persistence("Oversight lock poisoned".into()))?;
        let mut queue = requests::load(db, run)?;
        let previous_queue =
            serde_json::to_vec(&queue).map_err(|e| DaemonError::Serialization(e.to_string()))?;
        queue.closed |= DbCheckpointSink::load(db, run)?.is_none_or(|cp| !cp.status.resumable());
        queue.closed |= closing(db, run)?.is_some();
        for request in &mut queue.requests {
            let child_pending = request.proposals.iter().any(|p| unfinished(p.state));
            if unfinished(request.state) || child_pending {
                for proposal in &mut request.proposals {
                    if unfinished(proposal.state) {
                        proposal.state = State::ClosedUnhandled;
                    }
                }
                request.state = State::ClosedUnhandled;
                request.revision += 1;
                if !request.result_refs.iter().any(|r| r == "recovery:interrupted") {
                    request.result_refs.push("recovery:interrupted".into());
                }
            }
        }
        let queue_bytes =
            serde_json::to_vec(&queue).map_err(|e| DaemonError::Serialization(e.to_string()))?;
        let queue_key = format!("oversight:requests:{run}");
        if let Some(mut schedule) = scheduler::load(db, run)? {
            let original = serde_json::to_vec(&schedule)
                .map_err(|e| DaemonError::Serialization(e.to_string()))?;
            let cancelled = schedule
                .reviews
                .iter()
                .flat_map(|r| &r.proposals)
                .any(|p| p.kind == "CancelRun" && p.state == State::Applied);
            queue.closed |= cancelled;
            let queue_bytes = serde_json::to_vec(&queue)
                .map_err(|e| DaemonError::Serialization(e.to_string()))?;
            let unknown_cancel = cancelled && schedule.cancel_result.is_none();
            schedule.closed = true;
            for review in &mut schedule.reviews {
                if unknown_cancel {
                    for proposal in &mut review.proposals {
                        if proposal.kind == "CancelRun" && proposal.state == State::Applied {
                            proposal.reason="Cancellation was acknowledged before interruption; the final rollback outcome is unavailable. Recorded file phases must be inspected; this run cannot resume.".into();
                        }
                    }
                }
                for proposal in &mut review.proposals {
                    if unfinished(proposal.state) {
                        proposal.state = State::ClosedUnhandled;
                        proposal.reason="Interrupted before a durable result acknowledgement; old confirmation is invalid and no action will be replayed".into();
                    }
                }
                if matches!(review.status, Status::Pending | Status::Running) {
                    review.status = Status::Cancelled;
                    review.finished_at = Some(scheduler::now());
                    review.verdict = None;
                    review.model_verdict = None;
                    review.summary="Review interrupted during recovery; completed action references are retained, but no final model conclusion is available".into();
                }
            }
            let updated = serde_json::to_vec(&schedule)
                .map_err(|e| DaemonError::Serialization(e.to_string()))?;
            if original != updated || previous_queue != queue_bytes {
                db.put_pair_durable(
                    cf::EXECUTION_STATE,
                    format!("oversight:schedule:{run}").as_bytes(),
                    &updated,
                    queue_key.as_bytes(),
                    &queue_bytes,
                )?;
            }
        } else if previous_queue != queue_bytes {
            db.put_durable(cf::EXECUTION_STATE, queue_key.as_bytes(), &queue_bytes)?;
        }
    }
    Ok(())
}

/// A durable cancellation acknowledgement closes the run even if its final
/// checkpoint was interrupted. Resuming must not undo the user's stop decision.
pub(crate) fn ensure_resumable(db: &Db, run: Uuid) -> DaemonResult<()> {
    if closing(db, run)?.is_some() {
        return Err(DaemonError::Persistence("Run cancellation was acknowledged before interruption; inspect recorded file recovery and start a new run instead of replaying this run".into()));
    }
    Ok(())
}

pub(crate) fn mark_direct_cancel(db: &Db, run: Uuid) -> DaemonResult<()> {
    let _gate = db
        .oversight_gate
        .lock()
        .map_err(|_| DaemonError::Persistence("Oversight lock poisoned".into()))?;
    let key = format!("oversight:closing:{run}");
    if db.get(cf::EXECUTION_STATE, key.as_bytes())?.is_none() {
        let value =
            serde_json::json!({"run_id":run,"source":"user_direct","at_ms":scheduler::now()});
        db.put_durable(
            cf::EXECUTION_STATE,
            key.as_bytes(),
            &serde_json::to_vec(&value).map_err(|e| DaemonError::Serialization(e.to_string()))?,
        )?;
    }
    Ok(())
}
pub(crate) fn closing(db: &Db, run: Uuid) -> DaemonResult<Option<serde_json::Value>> {
    if let Some(bytes) =
        db.get(cf::EXECUTION_STATE, format!("oversight:closing:{run}").as_bytes())?
    {
        return serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| DaemonError::Serialization(e.to_string()));
    }
    Ok(scheduler::load(db, run)?
        .and_then(|s| {
            s.reviews
                .into_iter()
                .flat_map(|r| r.proposals)
                .find(|p| p.kind == "CancelRun" && p.state == State::Applied)
        })
        .map(
            |p| serde_json::json!({"run_id":run,"source":"supervisor","proposal_id":p.proposal_id}),
        ))
}
