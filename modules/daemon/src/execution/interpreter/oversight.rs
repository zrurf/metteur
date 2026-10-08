//! Explicit review gates. Only the interpreter can commit an approved edit here.
use super::Interpreter;
use crate::{
    DaemonError, DaemonResult,
    execution::context::ExecutionContext,
    oversight::{
        requests::State,
        scheduler::{self, Status},
    },
};
use metteur_shared::{Blueprint, Node, PinId, Value};
use std::{collections::HashMap, time::Duration};

impl Interpreter {
    pub(crate) async fn oversight_checkpoint(
        &mut self,
        node: &Node,
        blueprint: &Blueprint,
        ctx: &mut ExecutionContext,
    ) -> DaemonResult<HashMap<PinId, Value>> {
        let db = ctx.workspace_db.clone().ok_or_else(|| {
            DaemonError::Execution("Review gate requires a workspace database".into())
        })?;
        let asynchronous = match node.data.get("async") {
            None => false,
            Some(value) => value.as_bool().ok_or_else(|| {
                DaemonError::Execution("OversightCheckpoint async must be boolean".into())
            })?,
        };
        let mut id = scheduler::trigger(&db, ctx.run_id, "checkpoint", scheduler::now())?;
        if asynchronous {
            return Ok(outputs(
                node,
                id,
                "pending",
                "",
                "Review queued; no conclusion is available.",
            ));
        }
        loop {
            crate::execution::control::check_cancelled(ctx)?;
            if crate::replan::application::has_pending(ctx) {
                self.write_checkpoint(ctx)?;
            }
            let review = scheduler::load(&db, ctx.run_id)?
                .and_then(|s| s.reviews.into_iter().find(|r| r.review_id == id))
                .ok_or_else(|| DaemonError::Execution("Review gate lost its review".into()))?;
            if matches!(review.status, Status::Pending | Status::Running)
                || review.proposals.iter().any(|p| {
                    matches!(p.state, State::AwaitingConfirmation | State::ApprovedPendingApply)
                })
            {
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
            let status = serde_json::to_value(&review.status).unwrap().as_str().unwrap().to_owned();
            let verdict = review
                .model_verdict
                .as_deref()
                .or(review.verdict.as_deref().filter(|v| matches!(*v, "ok" | "concern")))
                .unwrap_or("");
            let handled_concern = handles_concern(node, blueprint);
            let actions_complete = review.proposals.iter().all(|p| p.state == State::Applied);
            if review.status == Status::Completed
                && actions_complete
                && (verdict == "ok" || (verdict == "concern" && handled_concern))
            {
                crate::execution::control::wait_while_paused(ctx).await?;
                return Ok(outputs(node, id, &status, verdict, &review.summary));
            }
            // This is a disposition of an unresolved gate, never an approval of
            // model conclusions or a cached permission for a future gate.
            let detail = serde_json::json!({"request_type":"oversight_gate", "run_id":ctx.run_id,"node_id":node.id,"review_id":id,"status":status,"verdict":verdict,"summary":review.summary,"allow":"Continue without changing the review conclusion", "deny":"Retry review", "confirmation_required":true});
            let subject = format!(
                "Review gate {id} requires disposition. Allow once to continue without validation; deny to retry. Stop cancels the run."
            );
            let response = crate::sandbox::request_user_approval(
                ctx,
                crate::sandbox::command_hash(&format!("{}:{id}", uuid::Uuid::new_v4())),
                &subject,
                detail,
                86400,
            )
            .await;
            crate::execution::control::check_cancelled(ctx)?;
            let response = response?;
            if response.scope != crate::sandbox::approval::Scope::Once {
                continue;
            }
            let continued = response.decision == crate::sandbox::approval::Decision::Allow;
            scheduler::disposition(&db, ctx.run_id, id, node.id, continued)?;
            let disposition = if continued {
                "User continued without validating the review."
            } else {
                "User requested another review."
            };
            ctx.audit("oversight.gate_disposition", serde_json::json!({"run_id":ctx.run_id,"node_id":node.id,"review_id":id,"disposition":disposition}));
            self.emit(super::ExecutionEvent::Message {
                node_id: node.id,
                message: format!(
                    "{disposition} Review: {id}; status: {status}; verdict: {verdict}"
                ),
            });
            if continued {
                crate::execution::control::wait_while_paused(ctx).await?;
                return Ok(outputs(
                    node,
                    id,
                    &status,
                    verdict,
                    &format!("{} {disposition}", review.summary),
                ));
            }
            id = scheduler::trigger(&db, ctx.run_id, "checkpoint", scheduler::now())?;
        }
    }
}
fn outputs(
    node: &Node,
    id: uuid::Uuid,
    status: &str,
    verdict: &str,
    notes: &str,
) -> HashMap<PinId, Value> {
    let values = [
        ("ReviewId", id.to_string()),
        ("Status", status.into()),
        ("Verdict", verdict.into()),
        ("Notes", notes.into()),
    ];
    values
        .into_iter()
        .filter_map(|(name, value)| {
            node.pins.iter().find(|p| p.name == name).map(|pin| (pin.id, Value::String(value)))
        })
        .collect()
}

// A disconnected switch or an unconnected case is not a disposition branch.
fn handles_concern(node: &Node, bp: &Blueprint) -> bool {
    let Some(verdict) = node.pins.iter().find(|p| p.name == "Verdict") else {
        return false;
    };
    bp.edges.iter().filter(|e| e.source_pin == verdict.id).any(|edge| {
        let Some(switch) = bp.node(edge.target_node).filter(|n| n.kind == "Switch") else {
            return false;
        };
        if !switch.pins.iter().any(|p| p.id == edge.target_pin && p.name == "Case") {
            return false;
        }
        let triggered = bp.edges.iter().any(|e| {
            e.source_node == node.id
                && e.target_node == switch.id
                && switch.pins.iter().any(|p| {
                    p.id == e.target_pin && p.pin_type == metteur_shared::PinType::ExecInput
                })
        });
        let branch = switch
            .pins
            .iter()
            .find(|p| p.name == "Case_concern")
            .or_else(|| switch.pins.iter().find(|p| p.name == "Default"));
        triggered && branch.is_some_and(|p| bp.edges.iter().any(|e| e.source_pin == p.id))
    })
}
#[cfg(test)]
mod tests {
    #[test]
    fn concern_requires_a_reachable_connected_disposition_branch() {
        let mut bp=metteur_shared::dsl::compile_draft_value(&serde_json::json!({"name":"Concern","nodes":{"g":{"kind":"OversightCheckpoint"},"b":{"kind":"Switch","Case":"$g.Verdict"},"e":{"kind":"End"}},"flow":["g -> b", "b.Default -> e"]})).unwrap();
        let gate = bp.nodes.iter().find(|n| n.kind == "OversightCheckpoint").unwrap().clone();
        assert!(super::handles_concern(&gate, &bp));
        let exec = bp
            .edges
            .iter()
            .position(|e| {
                e.source_node == gate.id
                    && gate.pins.iter().any(|p| {
                        p.id == e.source_pin && p.pin_type == metteur_shared::PinType::ExecOutput
                    })
            })
            .unwrap();
        let edge = bp.edges.remove(exec);
        assert!(!super::handles_concern(&gate, &bp));
        bp.edges.push(edge);
        let end = bp.nodes.iter().find(|n| n.kind == "End").unwrap().id;
        bp.edges.retain(|e| e.target_node != end);
        assert!(!super::handles_concern(&gate, &bp));
    }
}
