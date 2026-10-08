//! ForEach loop driving: enter, advance, and body re-arm.

use std::collections::HashMap;

use metteur_shared::{Blueprint, Node, NodeId, PinId, PinType, Value};

use crate::error::{DaemonError, DaemonResult};
use crate::execution::context::{ExecutionContext, ForEachState};
use crate::execution::nodes::value_input;
use crate::execution::tree::TreeNodeStatus;

use super::{ExecutionEvent, Interpreter};

impl Interpreter {
    /// Enters a ForEach loop over the `List` input.
    ///
    /// An empty list fires `Completed` immediately; otherwise the first item
    /// is exposed and the `Body` branch fires. Nested loops in the same frame
    /// are rejected (wrap the inner loop in a function).
    pub(crate) fn enter_foreach(
        &mut self,
        node_id: NodeId,
        node: &Node,
        inputs: &HashMap<PinId, Value>,
        blueprint: &Blueprint,
        ctx: &mut ExecutionContext,
    ) -> DaemonResult<()> {
        let depth = self.state.call_stack.len();
        if self.state.foreach_stack.last().is_some_and(|top| top.depth == depth) {
            return Err(DaemonError::Execution(
                "nested ForEach loops in one frame are not supported; wrap the inner loop in a function".to_string(),
            ));
        }
        let items = match value_input(node, inputs, "List")?.clone() {
            Value::List(items) => items,
            other => {
                return Err(DaemonError::Execution(format!(
                    "ForEach List input must be a list, got {other:?}"
                )));
            }
        };
        // The run loop already emitted `started` and built the tree node.
        self.active_scheduler_mut().mark_executed(node_id);
        if items.is_empty() {
            self.emit(ExecutionEvent::NodeFinished {
                node_id,
            });
            ctx.audit(
                "node.finished",
                serde_json::json!({ "node_id": node_id.to_string(), "kind": node.kind }),
            );
            self.tree_end(ctx, TreeNodeStatus::Done);
            self.commit_successors(blueprint, node_id, Some("Completed"), ctx)?;
            return Ok(());
        }
        self.state.foreach_stack.push(ForEachState {
            node_id,
            items,
            index: 0,
            depth,
            count: 1,
        });
        self.publish_foreach_item(node, ctx)?;
        self.emit(ExecutionEvent::NodeFinished {
            node_id,
        });
        ctx.audit(
            "node.finished",
            serde_json::json!({ "node_id": node_id.to_string(), "kind": node.kind }),
        );
        self.tree_end(ctx, TreeNodeStatus::Done);
        self.commit_successors(blueprint, node_id, Some("Body"), ctx)
    }

    /// Advances the innermost loop of the current frame after its body drains.
    ///
    /// Returns `true` when the loop continues (next item exposed, `Body`
    /// fired) or completes (`Completed` fired); `false` when no loop of this
    /// frame is active.
    pub(crate) fn advance_foreach(
        &mut self,
        blueprint: &Blueprint,
        ctx: &mut ExecutionContext,
    ) -> DaemonResult<bool> {
        let depth = self.state.call_stack.len();
        let Some(top) = self.state.foreach_stack.last() else {
            return Ok(false);
        };
        if top.depth != depth {
            return Ok(false);
        }
        let node_id = top.node_id;
        if top.index + 1 >= top.items.len() {
            self.state.foreach_stack.pop();
            self.commit_successors(blueprint, node_id, Some("Completed"), ctx)?;
            return Ok(true);
        }
        let max = ctx
            .config
            .as_ref()
            .and_then(|c| c.try_read().ok())
            .map(|cfg| cfg.execution.foreach_max_iterations)
            .unwrap_or(0);
        let top = self.state.foreach_stack.last_mut().expect("loop on top");
        top.index += 1;
        top.count += 1;
        if max != 0 && top.count > max {
            return Err(DaemonError::Execution(format!("ForEach loop exceeded {max} iterations")));
        }
        let node_id = top.node_id;
        self.unmark_exec_subgraph(blueprint, node_id, "Body")?;
        let node = blueprint
            .node(node_id)
            .cloned()
            .ok_or_else(|| DaemonError::Execution(format!("unknown node {node_id}")))?;
        self.publish_foreach_item(&node, ctx)?;
        self.commit_successors(blueprint, node_id, Some("Body"), ctx)?;
        Ok(true)
    }

    /// Un-marks the execution subgraph reachable from one named output pin.
    ///
    /// ForEach body iterations re-run their nodes every pass, so advancing
    /// the loop clears the completed marks (and stale queue entries) of the
    /// `Body` subgraph first.
    fn unmark_exec_subgraph(
        &mut self,
        blueprint: &Blueprint,
        node_id: NodeId,
        pin_name: &str,
    ) -> DaemonResult<()> {
        let start_pin = blueprint
            .node(node_id)
            .and_then(|n| n.pins.iter().find(|p| p.name == pin_name))
            .ok_or_else(|| DaemonError::Execution(format!("node missing pin {pin_name}")))?
            .id;
        let mut reached = vec![node_id];
        let mut stack = vec![node_id];
        while let Some(current) = stack.pop() {
            for edge in blueprint.outgoing_edges(current) {
                let source_pin = blueprint
                    .pin(edge.source_pin)
                    .ok_or_else(|| DaemonError::Execution("unknown source pin".to_string()))?;
                if source_pin.pin_type != PinType::ExecOutput {
                    continue;
                }
                if current == node_id && edge.source_pin != start_pin {
                    continue;
                }
                if !reached.contains(&edge.target_node) {
                    reached.push(edge.target_node);
                    stack.push(edge.target_node);
                }
            }
        }
        let body: Vec<_> = reached.iter().copied().filter(|id| *id != node_id).collect();
        self.view.invalidate(
            &body,
            &self.frame_trees,
            crate::execution::blackboard::ChangeKind::Iteration,
        );
        let sched = self.active_scheduler_mut();
        sched.dequeue_all(&reached);
        // The loop node itself stays completed; only its body re-runs.
        for id in reached.iter().filter(|id| **id != node_id) {
            sched.unmark_executed(*id);
        }
        Ok(())
    }

    /// Writes the current loop item and index into the ForEach node's outputs.
    fn publish_foreach_item(
        &mut self,
        node: &Node,
        ctx: &mut ExecutionContext,
    ) -> DaemonResult<()> {
        let top = self
            .state
            .foreach_stack
            .last()
            .ok_or_else(|| DaemonError::Execution("no active ForEach loop".to_string()))?;
        let item = top.items.get(top.index).cloned().unwrap_or(Value::Null);
        let index = top.index;
        let function = self.active_function();
        let mut outputs = Vec::new();
        for pin in &node.pins {
            if pin.pin_type != PinType::DataOutput {
                continue;
            }
            let value = if pin.name == "Iteration" {
                item.clone()
            } else if pin.name == "Index" {
                Value::Int(index as i64)
            } else {
                continue;
            };
            self.state.data_values.insert(pin.id, value.clone());
            outputs.push((pin.id, value));
        }
        self.emit(ExecutionEvent::NodeData {
            node_id: node.id,
            outputs,
            function,
        });
        ctx.audit(
            "foreach.item",
            serde_json::json!({ "node_id": node.id.to_string(), "index": index }),
        );
        Ok(())
    }

    /// Enqueues the targets of one named execution output pin.
    pub(super) fn fire_named_edge(
        &mut self,
        blueprint: &Blueprint,
        node_id: NodeId,
        pin_name: &str,
    ) -> DaemonResult<()> {
        let mut targets = Vec::new();
        for edge in blueprint.outgoing_edges(node_id) {
            let source_pin = blueprint
                .pin(edge.source_pin)
                .ok_or_else(|| DaemonError::Execution("unknown source pin".to_string()))?;
            if source_pin.pin_type == PinType::ExecOutput && source_pin.name == pin_name {
                self.view.traverse(blueprint, edge.id, self.frame_trees.clone());
                targets.push(edge.target_node);
            }
        }
        let sched = self.active_scheduler_mut();
        for target in targets {
            if !sched.is_executed(target) {
                sched.enqueue(target);
            }
        }
        Ok(())
    }
}
