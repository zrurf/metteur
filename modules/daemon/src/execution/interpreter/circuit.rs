//! A circuit has one supervisor owner and one concrete action confirmation.
use super::Interpreter;
use crate::{
    DaemonError, DaemonResult,
    execution::context::ExecutionContext,
    oversight::{requests::State, scheduler},
};
use metteur_shared::NodeId;
use std::time::Duration;
impl Interpreter {
    pub(crate) async fn supervised_circuit(
        &mut self,
        node: NodeId,
        ctx: &mut ExecutionContext,
    ) -> DaemonResult<()> {
        // Persist the failed result without dispatching its successors. The
        // fence prevents recovery from replaying a stale circuit approval.
        self.in_flight = Some(node);
        self.write_checkpoint(ctx)?;
        loop {
            let review = match &ctx.workspace_db {
                Some(db) => scheduler::circuit(db, ctx.run_id, node),
                None => {
                    Err(DaemonError::Execution("Supervisor requires a workspace database".into()))
                }
            };
            let mut reason = "Supervisor unavailable; no plan was applied.".to_string();
            if let (Ok(id), Some(db)) = (review, ctx.workspace_db.clone()) {
                loop {
                    crate::execution::control::check_cancelled(ctx)?;
                    if crate::replan::application::pending_review(ctx) == Some(id) {
                        self.queue_circuit_retry(node);
                        self.in_flight = None;
                        self.circuit_failures = 0;
                        self.write_checkpoint(ctx)?;
                        let state = scheduler::load(&db, ctx.run_id)?.ok_or_else(|| {
                            DaemonError::Execution("Circuit review unavailable".into())
                        })?;
                        if state.reviews.iter().find(|r| r.review_id == id).is_some_and(|r| {
                            r.proposals
                                .iter()
                                .any(|p| p.kind == "blueprint_edits" && p.state == State::Applied)
                        }) {
                            return Ok(());
                        }
                        return Err(DaemonError::Execution(
                            "Circuit proposal did not commit; no failed successor may run".into(),
                        ));
                    }
                    let state = scheduler::load(&db, ctx.run_id)?.ok_or_else(|| {
                        DaemonError::Execution("Circuit review unavailable".into())
                    })?;
                    let r = state.reviews.iter().find(|r| r.review_id == id).ok_or_else(|| {
                        DaemonError::Execution("Circuit review unavailable".into())
                    })?;
                    if !matches!(r.status, scheduler::Status::Pending | scheduler::Status::Running)
                    {
                        reason = format!("{} No circuit repair was applied.", r.summary);
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
            crate::execution::control::check_cancelled(ctx)?;
            let subject = format!(
                "Circuit stopped at {node}: {reason} Allow once to retry the failed node without claiming validation; deny to stop."
            );
            let detail = serde_json::json!({"request_type":"circuit_tripped","run_id":ctx.run_id,"node_id":node,"failures":self.circuit_failures,"summary":reason,"confirmation_required":true});
            let result = crate::sandbox::request_user_approval(
                ctx,
                crate::sandbox::command_hash(&uuid::Uuid::new_v4().to_string()),
                &subject,
                detail,
                86400,
            )
            .await;
            crate::execution::control::check_cancelled(ctx)?;
            let result = result?;
            if result.decision == crate::sandbox::approval::Decision::Deny {
                return Err(DaemonError::Execution("circuit tripped; aborted by user".into()));
            }
            if result.scope != crate::sandbox::approval::Scope::Once {
                continue;
            }
            ctx.audit("oversight.circuit_disposition",serde_json::json!({"run_id":ctx.run_id,"node_id":node,"action":"user_retry_without_validation"}));
            self.emit(super::ExecutionEvent::Message{node_id:node,message:"User requested a circuit retry without a model-approved repair or validation pass.".into()});
            self.queue_circuit_retry(node);
            self.in_flight = None;
            self.circuit_failures = 0;
            self.write_checkpoint(ctx)?;
            return Ok(());
        }
    }
    fn queue_circuit_retry(&mut self, node: NodeId) {
        self.view.invalidate(
            &[node],
            &self.frame_trees,
            crate::execution::blackboard::ChangeKind::CircuitBreak,
        );
        let sched = self.active_scheduler_mut();
        sched.unmark_executed(node);
        sched.dequeue_all(&[node]);
        sched.enqueue(node);
    }
}
