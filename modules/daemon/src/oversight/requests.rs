//! Trusted request intake. Model output has no authority fields or transition API.
use crate::{
    DaemonError, DaemonResult,
    execution::{DbCheckpointSink, RunStatus},
    storage::persistence::{Db, cf},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Query,
    Request,
    Complaint,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Intent {
    pub category: Category,
    pub note: String,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Received,
    Reviewing,
    AwaitingConfirmation,
    ApprovedPendingApply,
    Applied,
    Answered,
    Rejected,
    Failed,
    ClosedUnhandled,
}
impl State {
    fn finished(self) -> bool {
        matches!(
            self,
            Self::Applied | Self::Answered | Self::Rejected | Self::Failed | Self::ClosedUnhandled
        )
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposalResult {
    pub proposal_id: Uuid,
    pub state: State,
    pub result_refs: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestRecord {
    pub request_id: Uuid,
    pub run_id: Uuid,
    pub conversation_id: Uuid,
    pub at_ms: i64,
    pub original_text: String,
    pub concierge_note: String,
    pub category: Category,
    pub state: State,
    pub source: String,
    pub revision: u64,
    pub review_id: Option<Uuid>,
    pub proposals: Vec<ProposalResult>,
    pub result_refs: Vec<String>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Queue {
    pub closed: bool,
    pub requests: Vec<RequestRecord>,
}
fn key(run: Uuid) -> Vec<u8> {
    format!("oversight:requests:{run}").into_bytes()
}
pub fn load(db: &Db, run: Uuid) -> DaemonResult<Queue> {
    db.get(cf::EXECUTION_STATE, &key(run))?
        .map(|v| serde_json::from_slice(&v).map_err(|e| DaemonError::Serialization(e.to_string())))
        .transpose()
        .map(Option::unwrap_or_default)
}
fn save(db: &Db, run: Uuid, queue: &Queue) -> DaemonResult<()> {
    db.put_durable(
        cf::EXECUTION_STATE,
        &key(run),
        &serde_json::to_vec(queue).map_err(|e| DaemonError::Serialization(e.to_string()))?,
    )
}
/// The service supplies identity and original text; an LLM may supply only Intent.
pub fn receive(
    db: &Db,
    run: Uuid,
    conversation: Uuid,
    id: Uuid,
    text: &str,
    intent: Intent,
) -> DaemonResult<RequestRecord> {
    if text.trim().is_empty() || text.len() > 16384 || intent.note.len() > 8192 {
        return Err(DaemonError::Execution("invalid request size".into()));
    }
    let _guard = db
        .oversight_gate
        .lock()
        .map_err(|_| DaemonError::Persistence("oversight lock poisoned".into()))?;
    let identity_key = format!("oversight:request-id:{id}");
    let identity = serde_json::to_vec(&(run, conversation, text))
        .map_err(|e| DaemonError::Serialization(e.to_string()))?;
    if db
        .get(cf::EXECUTION_STATE, identity_key.as_bytes())?
        .is_some_and(|stored| stored != identity)
    {
        return Err(DaemonError::Execution(
            "request id belongs to a different run, conversation or message".into(),
        ));
    }
    let mut queue = load(db, run)?;
    if let Some(existing) = queue.requests.iter().find(|r| r.request_id == id) {
        if existing.conversation_id != conversation || existing.original_text != text {
            return Err(DaemonError::Execution(
                "request id already bound to different content".into(),
            ));
        }
        return Ok(existing.clone());
    }
    let checkpoint = DbCheckpointSink::load(db, run)?
        .ok_or_else(|| DaemonError::NotFound("blueprint run".into()))?;
    if queue.closed || checkpoint.status != RunStatus::Running {
        return Err(DaemonError::Execution("run is not accepting requests".into()));
    }
    let record = RequestRecord {
        request_id: id,
        run_id: run,
        conversation_id: conversation,
        at_ms: chrono::Utc::now().timestamp_millis(),
        original_text: text.into(),
        concierge_note: intent.note,
        category: intent.category,
        state: State::Received,
        source: "concierge_forwarded".into(),
        revision: 1,
        review_id: None,
        proposals: vec![],
        result_refs: vec![],
    };
    // Binding may survive a failed receipt, but is never itself acknowledged.
    // A retry with the original identity can finish the durable receipt.
    db.put_durable(cf::EXECUTION_STATE, identity_key.as_bytes(), &identity)?;
    queue.requests.push(record.clone());
    save(db, run, &queue)?;
    super::scheduler::queue_locked(db, run, &queue)?;
    Ok(record)
}
/// Called while the same database gate excludes intake. A terminal marker is
/// durable before its checkpoint; an interrupted close can never acknowledge
/// a new request for a run that has already stopped accepting work.
pub(crate) fn close_locked(db: &Db, run: Uuid) -> DaemonResult<()> {
    let mut queue = load(db, run)?;
    queue.closed = true;
    for request in &mut queue.requests {
        if !request.state.finished() {
            for proposal in &mut request.proposals { if !proposal.state.finished() {proposal.state=State::ClosedUnhandled;} }
            request.state = State::ClosedUnhandled;
            request.revision += 1;
        }
    }
    save(db, run, &queue)
}
/// Future trusted consumers update by revision, never by deserializing a model's
/// proposed record. Intake provenance and user text are immutable.
pub fn transition(
    db: &Db,
    run: Uuid,
    id: Uuid,
    revision: u64,
    next: Transition,
) -> DaemonResult<RequestRecord> {
    let _guard = db
        .oversight_gate
        .lock()
        .map_err(|_| DaemonError::Persistence("oversight lock poisoned".into()))?;
    let mut queue = load(db, run)?;
    if queue.closed {
        return Err(DaemonError::Execution("request queue closed".into()));
    }
    let r = queue
        .requests
        .iter_mut()
        .find(|r| r.request_id == id)
        .ok_or_else(|| DaemonError::NotFound("request".into()))?;
    if r.revision != revision || r.state.finished() {
        return Err(DaemonError::Execution("stale request transition".into()));
    }
    match next {
        Transition::Review(review) if r.state == State::Received => {
            r.review_id = Some(review);
            r.state = State::Reviewing;
        }
        Transition::Propose(ids) if r.state == State::Reviewing && !ids.is_empty() => {
            let unique: std::collections::HashSet<_> = ids.iter().collect();
            if unique.len() != ids.len() {
                return Err(DaemonError::Execution("duplicate proposal id".into()));
            }
            r.proposals = ids
                .into_iter()
                .map(|proposal_id| ProposalResult {
                    proposal_id,
                    state: State::AwaitingConfirmation,
                    result_refs: vec![],
                })
                .collect();
            r.state = State::AwaitingConfirmation;
        }
        Transition::Proposal {
            id,
            state,
            refs,
        } => {
            let p = r
                .proposals
                .iter_mut()
                .find(|p| p.proposal_id == id)
                .ok_or_else(|| DaemonError::NotFound("proposal".into()))?;
            let valid = matches!(
                (p.state, state),
                (State::AwaitingConfirmation, State::ApprovedPendingApply | State::Rejected)
                    | (State::ApprovedPendingApply, State::Applied | State::Failed)
            );
            if !valid || (state == State::Applied && refs.is_empty()) {
                return Err(DaemonError::Execution("invalid proposal transition".into()));
            }
            p.state = state;
            p.result_refs = refs;
            r.state = if r.proposals.iter().all(|p| p.state == State::Applied) {
                State::Applied
            } else if r.proposals.iter().all(|p| p.state == State::Rejected) {
                State::Rejected
            } else if r.proposals.iter().all(|p| p.state.finished()) {
                State::Failed
            } else if r.proposals.iter().any(|p| p.state == State::AwaitingConfirmation) {
                State::AwaitingConfirmation
            } else {
                State::ApprovedPendingApply
            };
        }
        Transition::Finish {
            state,
            refs,
        } if matches!(r.state, State::Received | State::Reviewing)
            && matches!(state, State::Answered | State::Rejected | State::Failed) =>
        {
            if state == State::Answered && refs.is_empty() {
                return Err(DaemonError::Execution("answer needs evidence".into()));
            }
            r.state = state;
            r.result_refs = refs;
        }
        _ => return Err(DaemonError::Execution("invalid request transition".into())),
    }
    r.revision += 1;
    let result = r.clone();
    save(db, run, &queue)?;
    Ok(result)
}
/// No Deserialize implementation: this is exclusively a trusted service API.
pub enum Transition {
    Review(Uuid),
    Propose(Vec<Uuid>),
    Proposal {
        id: Uuid,
        state: State,
        refs: Vec<String>,
    },
    Finish {
        state: State,
        refs: Vec<String>,
    },
}
