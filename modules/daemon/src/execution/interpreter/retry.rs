//! Validation retry and the circuit breaker.

use std::collections::HashMap;

use metteur_shared::{Node, NodeId, PinId, Value};

use crate::error::DaemonResult;
use crate::execution::context::{ExecutionContext, RetryMark};

use super::{ExecutionEvent, Interpreter};

impl Interpreter {
    /// Handles validation retry for a failed validator node.
    ///
    /// Returns `true` when the node will re-run: the file mutations recorded
    /// since its last pass are rolled back (WAL before-images), the executed
    /// segment is re-queued in completion order, and the failure does not
    /// count toward the circuit breaker. Returns `false` when there is no
    /// retry budget left (or none configured) and the circuit breaker should
    /// see the failure. A passing validator refreshes its rollback mark.
    ///
    /// Runs before execution edges fire, so successors of the failed attempt
    /// are never enqueued with stale data.
    pub(crate) fn maybe_retry(
        &mut self,
        node_id: NodeId,
        node: &Node,
        outputs: &HashMap<PinId, Value>,
        ctx: &mut ExecutionContext,
    ) -> DaemonResult<bool> {
        // `LspCheck` is a deterministic validation node: a failing check
        // retries the segment exactly like a failing `Validator`.
        if !matches!(node.kind.as_str(), "Validator" | "LspCheck") {
            return Ok(false);
        }
        let failed = circuit_failed(node, outputs);
        let mut mark = *self.state.validation_marks.entry(node_id).or_default();
        // The completion order is not monotonic: ForEach iterations and fresh
        // function invocations un-mark and re-run nodes, which can leave a
        // stored `order_mark` past the end of the current order. A stale mark
        // would roll back a bounded log prefix yet re-queue an empty segment,
        // silently accepting the failure. Fall back to the whole frame.
        if mark.order_mark > self.active_scheduler().order_len() {
            mark = RetryMark {
                log_mark: 0,
                order_mark: 0,
            };
            self.state.validation_marks.insert(node_id, mark);
        }
        if !failed {
            let log_mark = ctx.transaction_log.mark();
            let order_mark = self.active_scheduler().order_len();
            self.state.validation_marks.insert(
                node_id,
                RetryMark {
                    log_mark,
                    order_mark,
                },
            );
            self.state.attempt_counts.remove(&node_id);
            return Ok(false);
        }

        let retry = node.data.get("retry");
        let max_attempts = retry
            .and_then(|r| r.get("max_attempts"))
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            .or_else(|| {
                ctx.config
                    .as_ref()
                    .and_then(|c| c.try_read().ok())
                    .map(|cfg| cfg.execution.validation_max_attempts)
            })
            .unwrap_or(1);
        let attempts = self.state.attempt_counts.entry(node_id).or_insert(0);
        if max_attempts <= 1 || *attempts >= max_attempts - 1 {
            return Ok(false);
        }
        *attempts += 1;
        let attempt = *attempts + 1;
        self.circuit_failures = 0;

        let rollback_enabled =
            retry.and_then(|r| r.get("rollback")).and_then(|v| v.as_bool()).unwrap_or(true);
        let segment = self.active_scheduler().order_from(mark.order_mark);
        self.view.invalidate(
            &segment,
            &self.frame_trees,
            if rollback_enabled {
                crate::execution::blackboard::ChangeKind::Rollback
            } else {
                crate::execution::blackboard::ChangeKind::Retry
            },
        );
        if rollback_enabled {
            let undone = ctx.transaction_log.rollback_after(mark.log_mark)?;
            ctx.audit(
                "execution.validation_rollback",
                serde_json::json!({
                    "run_id": ctx.run_id.to_string(),
                    "node_id": node_id.to_string(),
                    "undone": undone,
                }),
            );
            self.emit(ExecutionEvent::Message {
                node_id,
                message: format!(
                    "validation failed: rolled back {undone} file mutation(s), retrying attempt {attempt}/{max_attempts}"
                ),
            });
        } else {
            self.emit(ExecutionEvent::Message {
                node_id,
                message: format!("validation failed: retrying attempt {attempt}/{max_attempts}"),
            });
        }

        // Re-queue the executed segment (completion order) plus this
        // validator so the whole attempt re-runs on the restored files.
        // A validator that passed inside the rolled-back segment loses that
        // pass; clamp its mark to this segment so a later failure of it
        // re-runs the full (superset) segment instead of a stale suffix.
        for (vid, m) in self.state.validation_marks.iter_mut() {
            if *vid != node_id && m.order_mark > mark.order_mark {
                *m = mark;
            }
        }
        {
            let sched = self.active_scheduler_mut();
            // Drain stale queued duplicates first: a node fed by both an exec
            // edge and a data edge sits in the queue twice, and un-marking
            // would otherwise resurrect the stale copy.
            sched.dequeue_all(&segment);
            for id in &segment {
                sched.unmark_executed(*id);
            }
            for id in &segment {
                sched.enqueue(*id);
            }
        }
        self.write_checkpoint(ctx)?;
        Ok(true)
    }

    /// Counts consecutive validator/judge failures and, past the configured
    /// threshold, trips the circuit breaker (replan + rerun).
    pub(crate) async fn maybe_circuit_break(
        &mut self,
        node_id: NodeId,
        node: &Node,
        outputs: &HashMap<PinId, Value>,
        ctx: &mut ExecutionContext,
    ) -> DaemonResult<bool> {
        let failed = circuit_failed(node, outputs);
        self.circuit_failures = if failed {
            self.circuit_failures + 1
        } else {
            0
        };
        let threshold = ctx
            .config
            .as_ref()
            .and_then(|c| c.try_read().ok())
            .map(|cfg| cfg.execution.circuit_break_after)
            .unwrap_or(0);
        if threshold == 0 || self.circuit_failures < threshold {
            return Ok(false);
        }
        self.supervised_circuit(node_id, ctx).await?;
        Ok(true)
    }
}

/// Returns whether a validation node's output reports failure.
///
/// `LspCheck` participates here so that a failing deterministic language-server
/// check gets the same rollback/retry and circuit-breaker treatment as a
/// `Validator`: the edit that broke the code is rolled back and retried.
fn circuit_failed(node: &Node, outputs: &HashMap<PinId, Value>) -> bool {
    if !matches!(node.kind.as_str(), "Validator" | "Judge" | "LspCheck") {
        return false;
    }
    let result_pin = match node.kind.as_str() {
        "Validator" | "LspCheck" => "Passed",
        _ => "Success",
    };
    outputs.iter().any(|(id, v)| {
        node.pins.iter().any(|p| p.id == *id && p.name == result_pin) && v.as_bool() == Some(false)
    })
}
