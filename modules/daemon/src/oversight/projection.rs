//! Shared read projection for Requests, Reviews and CLI; it grants no authority.
use super::{diagnostic::{Category, Diagnostic, Stage}, requests, scheduler};
use crate::{DaemonResult, storage::persistence::Db};
use serde_json::{Value, json};

pub(crate) fn review(db: &Db, record: &scheduler::Review, queue: &requests::Queue) -> DaemonResult<Value> {
    let mut value = serde_json::to_value(record).expect("serializable review");
    if record.diagnostic.is_none() && matches!(record.status, scheduler::Status::Failed | scheduler::Status::TimedOut | scheduler::Status::BudgetExhausted | scheduler::Status::Cancelled) {
        value["diagnostic"] = json!(Diagnostic::new(Category::UnknownLegacy, Stage::Unknown, record.run_id, record.review_id));
    }
    for (projected, proposal) in value["proposals"].as_array_mut().into_iter().flatten().zip(&record.proposals) {
        if proposal.diagnostic.is_none() && matches!(proposal.state, requests::State::Failed | requests::State::Rejected | requests::State::ClosedUnhandled) {
            let mut detail = Diagnostic::new(Category::UnknownLegacy, Stage::Unknown, record.run_id, record.review_id);
            detail.proposal_id = Some(proposal.proposal_id);
            projected["diagnostic"] = json!(detail);
        }
        projected["original_requests"] = json!(queue.requests.iter().filter(|r| {
            r.run_id == record.run_id && r.review_id == Some(record.review_id)
                && proposal.source_request_ids.contains(&r.request_id)
        }).map(|r| json!({"request_id":r.request_id,"source":r.source,"original_text":r.original_text})).collect::<Vec<_>>());
        projected["result_version"] = if proposal.kind == "blueprint_edits" {
            json!(crate::replan::application::result_version(db, record.run_id, record.review_id, proposal.proposal_id)?)
        } else { Value::Null };
    }
    Ok(value)
}
