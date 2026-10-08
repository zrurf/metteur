//! Data-flow and execution-edge traversal of the active frame.

use std::collections::HashMap;

use metteur_shared::{Blueprint, NodeId, PinId, PinType, Value};

use crate::error::{DaemonError, DaemonResult};
use crate::execution::nodes::{json_to_value, value_to_string};

use super::Interpreter;

impl Interpreter {
    /// Follows a node's execution output edges and wakes data consumers in
    /// the active frame.
    pub(crate) fn fire_edges(
        &mut self,
        blueprint: &Blueprint,
        node_id: NodeId,
    ) -> DaemonResult<()> {
        let next_nodes = self.follow_exec_edges(blueprint, node_id)?;
        let ready_consumers: Vec<NodeId> = self
            .data_targets(blueprint, node_id)
            .into_iter()
            .filter(|t| self.data_inputs_ready(blueprint, *t))
            .collect();
        let sched = self.active_scheduler_mut();
        for next in next_nodes {
            if !sched.is_executed(next) {
                sched.enqueue(next);
            }
        }
        for target in ready_consumers {
            if sched.is_triggered(target) && !sched.is_executed(target) {
                sched.enqueue(target);
            }
        }
        Ok(())
    }

    /// Returns whether all data inputs of `node_id` have produced values.
    pub(crate) fn data_inputs_ready(&self, blueprint: &Blueprint, node_id: NodeId) -> bool {
        blueprint.incoming_edges(node_id).iter().all(|edge| {
            let source_pin = match blueprint.pin(edge.source_pin) {
                Some(pin) => pin,
                None => return false,
            };
            if source_pin.pin_type != PinType::DataOutput {
                return true;
            }
            self.state.data_values.contains_key(&edge.source_pin)
        })
    }

    /// Returns the nodes that consume data produced by `node_id`.
    fn data_targets(&self, blueprint: &Blueprint, node_id: NodeId) -> Vec<NodeId> {
        let mut targets = Vec::new();
        for edge in blueprint.outgoing_edges(node_id) {
            let source_pin = match blueprint.pin(edge.source_pin) {
                Some(pin) => pin,
                None => continue,
            };
            if source_pin.pin_type == PinType::DataOutput {
                targets.push(edge.target_node);
            }
        }
        targets
    }

    /// Gathers the data input values for a node from its incoming data edges.
    ///
    /// Inputs without an incoming edge fall back to the pin's default value,
    /// or to null when the pin is optional; genuinely missing inputs stay
    /// absent so the executor reports them.
    pub(crate) fn gather_inputs(
        &self,
        blueprint: &Blueprint,
        node_id: NodeId,
    ) -> DaemonResult<HashMap<PinId, Value>> {
        let mut inputs = HashMap::new();
        let signature = blueprint
            .node(node_id)
            .and_then(|node| self.registry.node_executor(&node.kind))
            .map(|executor| executor.signature());
        for edge in blueprint.incoming_edges(node_id) {
            let source_pin = blueprint
                .pin(edge.source_pin)
                .ok_or_else(|| DaemonError::Execution("unknown source pin".to_string()))?;
            if source_pin.pin_type != PinType::DataOutput {
                continue;
            }
            let value = self.state.data_values.get(&edge.source_pin).cloned().ok_or_else(|| {
                DaemonError::Execution(format!(
                    "missing data value for pin {} while gathering inputs of node {node_id}",
                    edge.source_pin
                ))
            })?;
            // Coerce toward the pin's declared type so a downstream executor
            // receives the type it declared. Validation already rejects
            // incompatible wirings, so a failure here means the graph changed
            // under a running plan (a replan) and is worth surfacing.
            let value = match blueprint.pin(edge.target_pin) {
                Some(target_pin) => match metteur_shared::coerce(
                    &value,
                    signature
                        .as_ref()
                        .and_then(|s| s.pin(target_pin))
                        .map(|p| &p.data_type)
                        .unwrap_or(&target_pin.data_type),
                ) {
                    Some(coerced) => coerced,
                    None => {
                        return Err(DaemonError::Execution(format!(
                            "node {node_id} input '{}' expects {} but the incoming value is {value:?}",
                            target_pin.name, target_pin.data_type
                        )));
                    }
                },
                None => value,
            };
            inputs.insert(edge.target_pin, value);
        }
        if let Some(node) = blueprint.node(node_id) {
            for pin in node.pins.iter().filter(|p| p.pin_type == PinType::DataInput) {
                if inputs.contains_key(&pin.id) {
                    continue;
                }
                // Inline constants predate canonical signatures and must win
                // over newly supplied defaults (or an optional null).
                if let Some(value) = metteur_shared::node_catalog::inline_value(node, pin) {
                    inputs.insert(pin.id, json_to_value(value));
                } else if let Some(default) = pin.default.as_ref().or_else(|| {
                    signature.as_ref().and_then(|s| s.pin(pin)).and_then(|p| p.default.as_ref())
                }) {
                    inputs.insert(pin.id, json_to_value(default));
                } else if pin.optional && !matches!(node.kind.as_str(), "ListCreate" | "Tool") {
                    inputs.insert(pin.id, Value::Null);
                }
            }
        }
        Ok(inputs)
    }

    /// Returns the target nodes of a node's execution output edges.
    ///
    /// `Branch` follows the `True`/`False` edge matching its `Result`;
    /// `Switch` follows the `Case_<value>` edge matching its `Result`,
    /// falling back to `Default`; `RequestApproval` follows `Approved` or
    /// `Denied` from its `Allowed` output. Any other node fans out to all of
    /// its execution output edges.
    fn follow_exec_edges(
        &mut self,
        blueprint: &Blueprint,
        node_id: NodeId,
    ) -> DaemonResult<Vec<NodeId>> {
        let node = blueprint
            .node(node_id)
            .ok_or_else(|| DaemonError::Execution(format!("unknown node {node_id}")))?;

        let take = match node.kind.as_str() {
            "Branch" => {
                let result = self.branch_result(blueprint, node_id)?;
                Some(if result {
                    "True".to_string()
                } else {
                    "False".to_string()
                })
            }
            "Switch" => Some(self.switch_take(blueprint, node_id)?),
            "RequestApproval" => {
                let allowed = self.approval_result(blueprint, node_id)?;
                Some(if allowed {
                    "Approved".to_string()
                } else {
                    "Denied".to_string()
                })
            }
            _ => None,
        };

        let mut next = Vec::new();
        for edge in blueprint.outgoing_edges(node_id) {
            let source_pin = blueprint
                .pin(edge.source_pin)
                .ok_or_else(|| DaemonError::Execution("unknown source pin".to_string()))?;
            if source_pin.pin_type != PinType::ExecOutput {
                continue;
            }
            if let Some(take) = &take
                && source_pin.name != *take
            {
                continue;
            }
            self.view.traverse(blueprint, edge.id, self.frame_trees.clone());
            next.push(edge.target_node);
        }
        Ok(next)
    }

    /// Reads the boolean result of a `Branch` node from its `Result` pin.
    fn branch_result(&self, blueprint: &Blueprint, node_id: NodeId) -> DaemonResult<bool> {
        let node = blueprint
            .node(node_id)
            .ok_or_else(|| DaemonError::Execution(format!("unknown node {node_id}")))?;
        let pin =
            node.pins.iter().find(|p| p.name == "Result").ok_or_else(|| {
                DaemonError::Execution("branch node missing Result pin".to_string())
            })?;
        self.state
            .data_values
            .get(&pin.id)
            .and_then(|v| v.as_bool())
            .ok_or_else(|| DaemonError::Execution("branch node missing Result value".to_string()))
    }

    /// Returns the execution output pin taken by a `Switch` node.
    ///
    /// The `Result` value selects the `Case_<value>` pin; when no such pin is
    /// wired, `Default` is taken so unmatched cases are an empty branch rather
    /// than an error.
    fn switch_take(&self, blueprint: &Blueprint, node_id: NodeId) -> DaemonResult<String> {
        let node = blueprint
            .node(node_id)
            .ok_or_else(|| DaemonError::Execution(format!("unknown node {node_id}")))?;
        let pin =
            node.pins.iter().find(|p| p.name == "Result").ok_or_else(|| {
                DaemonError::Execution("switch node missing Result pin".to_string())
            })?;
        let value = self.state.data_values.get(&pin.id).ok_or_else(|| {
            DaemonError::Execution("switch node missing Result value".to_string())
        })?;
        let candidate = format!("Case_{}", value_to_string(value));
        let wired: Vec<&str> = blueprint
            .outgoing_edges(node_id)
            .iter()
            .filter_map(|edge| blueprint.pin(edge.source_pin))
            .filter(|pin| pin.pin_type == PinType::ExecOutput)
            .map(|pin| pin.name.as_str())
            .collect();
        if wired.contains(&candidate.as_str()) {
            Ok(candidate)
        } else {
            Ok("Default".to_string())
        }
    }

    /// Reads the boolean outcome of a `RequestApproval` node.
    fn approval_result(&self, blueprint: &Blueprint, node_id: NodeId) -> DaemonResult<bool> {
        let node = blueprint
            .node(node_id)
            .ok_or_else(|| DaemonError::Execution(format!("unknown node {node_id}")))?;
        let pin = node.pins.iter().find(|p| p.name == "Allowed").ok_or_else(|| {
            DaemonError::Execution("approval node missing Allowed pin".to_string())
        })?;
        self.state.data_values.get(&pin.id).and_then(|v| v.as_bool()).ok_or_else(|| {
            DaemonError::Execution("approval node missing Allowed value".to_string())
        })
    }
}

/// Returns whether the blueprint's execution graph contains a cycle.
///
/// The graph is built from execution output edges; a cycle means execution
/// could never terminate.
pub(crate) fn has_exec_cycle(blueprint: &Blueprint) -> bool {
    // 0 = unvisited, 1 = in progress, 2 = done.
    let mut state: HashMap<NodeId, u8> = HashMap::new();
    for node in &blueprint.nodes {
        if visit_exec(node.id, blueprint, &mut state) {
            return true;
        }
    }
    false
}

/// Depth-first visit of the execution graph; returns true on a cycle.
fn visit_exec(node_id: NodeId, blueprint: &Blueprint, state: &mut HashMap<NodeId, u8>) -> bool {
    match state.get(&node_id) {
        Some(1) => return true,
        Some(2) => return false,
        _ => {}
    }
    state.insert(node_id, 1);
    for edge in blueprint.outgoing_edges(node_id) {
        let source_pin = match blueprint.pin(edge.source_pin) {
            Some(pin) => pin,
            None => continue,
        };
        if source_pin.pin_type != PinType::ExecOutput {
            continue;
        }
        if visit_exec(edge.target_node, blueprint, state) {
            return true;
        }
    }
    state.insert(node_id, 2);
    false
}
