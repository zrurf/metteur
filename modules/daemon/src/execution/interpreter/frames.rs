//! Function frame management: enter, finish, and active-frame accessors.

use std::collections::HashMap;

use metteur_shared::{
    Blueprint, FUNCTION_ENTRY_KIND, FUNCTION_EXIT_KIND, Node, NodeId, PinId, PinType, Value,
};

use crate::error::{DaemonError, DaemonResult};
use crate::execution::context::{ExecutionContext, Frame, FunctionBody, Scheduler};
use crate::execution::tree::{TreeNodeKind, TreeNodeStatus};

use super::checkpointing::now_millis;
use super::edges::has_exec_cycle;
use super::{ExecutionEvent, Interpreter, MAX_FUNCTION_DEPTH};

impl Interpreter {
    /// Returns the id of the innermost function currently on the stack.
    pub(crate) fn active_function(&self) -> Option<uuid::Uuid> {
        match self.state.call_stack.last() {
            Some(frame) => frame.function.as_ref().map(|b| b.id),
            None => None,
        }
    }

    /// Returns a reference to the scheduler of the active frame.
    pub(crate) fn active_scheduler(&self) -> &Scheduler {
        match self.state.call_stack.last() {
            Some(frame) if frame.function.is_some() => &frame.function.as_ref().unwrap().scheduler,
            _ => &self.scheduler,
        }
    }

    /// Returns a mutable reference to the scheduler of the active frame.
    pub(crate) fn active_scheduler_mut(&mut self) -> &mut Scheduler {
        match self.state.call_stack.last_mut() {
            Some(frame) if frame.function.is_some() => {
                &mut frame.function.as_mut().unwrap().scheduler
            }
            _ => &mut self.scheduler,
        }
    }

    /// Number of function frames currently on the stack.
    fn function_depth(&self) -> u32 {
        self.state.call_stack.iter().filter(|f| f.function.is_some()).count() as u32
    }

    /// Enters the function named by `node.data.function`, binding signature
    /// inputs to the caller's data input values and pushing a new frame.
    pub(crate) async fn enter_function(
        &mut self,
        caller_id: NodeId,
        caller_node: &Node,
        inputs: &HashMap<PinId, Value>,
        ctx: &mut ExecutionContext,
    ) -> DaemonResult<()> {
        let name = caller_node.data.get("function").and_then(|v| v.as_str()).ok_or_else(|| {
            DaemonError::Execution("CallFunction requires data.function".to_string())
        })?;
        let entry = self.registry.function(name).ok_or_else(|| {
            DaemonError::Execution(format!("function '{name}' is not registered"))
        })?;
        self.registry.validate_addon_nodes(&entry.body)?;
        if self.function_depth() >= MAX_FUNCTION_DEPTH {
            return Err(DaemonError::Execution(
                "function nesting depth limit exceeded".to_string(),
            ));
        }
        if has_exec_cycle(&entry.body) {
            return Err(DaemonError::Execution(format!(
                "function '{name}' contains an execution cycle"
            )));
        }
        let entry_node =
            entry.body.nodes.iter().find(|n| n.kind == FUNCTION_ENTRY_KIND).cloned().ok_or_else(
                || DaemonError::Execution(format!("function '{name}' has no entry node")),
            )?;
        // Bind each signature input to the caller's matching data pin.
        for fp in &entry.signature.inputs {
            let caller_pin = caller_node
                .pins
                .iter()
                .find(|p| p.name == fp.name && p.pin_type == PinType::DataInput)
                .ok_or_else(|| {
                    DaemonError::Execution(format!(
                        "CallFunction '{name}' is missing input pin '{}'",
                        fp.name
                    ))
                })?;
            let value = inputs.get(&caller_pin.id).cloned().ok_or_else(|| {
                DaemonError::Execution(format!(
                    "CallFunction '{name}' input '{}' is not wired",
                    fp.name
                ))
            })?;
            if entry.source == metteur_shared::FunctionSource::Addon
                && !(fp.optional && matches!(value, Value::Null))
                && !crate::addon::functions::value_matches(&value, &fp.data_type)
            {
                return Err(DaemonError::Execution("Addon function input type mismatch".into()));
            }
            let entry_pin = entry_node
                .pins
                .iter()
                .find(|p| p.name == fp.name && p.pin_type == PinType::DataOutput)
                .ok_or_else(|| {
                    DaemonError::Execution(format!(
                        "function '{name}' entry lacks output pin '{}'",
                        fp.name
                    ))
                })?;
            self.state.data_values.insert(entry_pin.id, value);
        }

        let mut sched = Scheduler::default();
        sched.seed(entry_node.id);
        self.active_scheduler_mut().mark_executed(caller_id);
        self.state.call_stack.push(Frame {
            node_id: caller_id,
            pc: 0,
            function: Some(FunctionBody {
                id: entry.id,
                scheduler: sched,
            }),
        });
        // A fresh variable frame: locals stay inside the function body.
        ctx.variables.push(HashMap::new());
        // A tree node for the function body, parented under the caller.
        let func_tree = self.tree.spawn(
            self.current_tree.as_deref(),
            TreeNodeKind::Function(name.to_string()),
            name.to_string(),
            now_millis(),
        );
        self.frame_trees.push(func_tree);
        if let Some(record) = self.view.invocations.last_mut() {
            record.owned_frame = Some(self.frame_trees.clone());
        }
        ctx.audit(
            "function.enter",
            serde_json::json!({
                "caller": caller_id.to_string(),
                "function": name,
                "depth": self.function_depth(),
            }),
        );
        // The parent caller is executed and the new frame already owns its
        // seeded entry, variable scope and tree before it becomes resumable.
        self.write_checkpoint(ctx)?;
        Ok(())
    }

    /// Pops a drained function frame, mapping its exit data inputs onto the
    /// caller's data output pins and driving the caller's execution edges.
    pub(crate) async fn finish_function(
        &mut self,
        frame: &Frame,
        body: &Blueprint,
        ctx: &mut ExecutionContext,
    ) -> DaemonResult<()> {
        let caller_id = frame.node_id;
        let exit_node =
            body.nodes.iter().find(|n| n.kind == FUNCTION_EXIT_KIND).cloned().ok_or_else(|| {
                DaemonError::Execution("function body has no exit node".to_string())
            })?;
        let exit_inputs = self.gather_inputs(body, exit_node.id)?;

        // Resolve the caller node in the outer blueprint.
        let outer_bp = match self.active_function() {
            Some(fn_id) => {
                self.registry
                    .function_by_id(fn_id)
                    .ok_or_else(|| {
                        DaemonError::Execution(format!("function {fn_id} not in registry"))
                    })?
                    .body
            }
            None => {
                let root = self
                    .shared_blueprint
                    .clone()
                    .ok_or_else(|| DaemonError::Execution("no root blueprint".to_string()))?;
                root.read().clone()
            }
        };
        let caller = outer_bp
            .node(caller_id)
            .cloned()
            .ok_or_else(|| DaemonError::Execution(format!("caller node {caller_id} not found")))?;

        // Map exit data inputs to the caller's data output pins by name.
        let mut caller_outputs: Vec<(PinId, Value)> = Vec::new();
        for pin in &exit_node.pins {
            if pin.pin_type != PinType::DataInput {
                continue;
            }
            let value = exit_inputs.get(&pin.id).cloned().ok_or_else(|| {
                DaemonError::Execution(format!("missing function output '{}'", pin.name))
            })?;
            if caller.data.get(metteur_shared::node_catalog::addon::FUNCTION_BINDING_KEY).is_some()
                && !(pin.optional && matches!(value, Value::Null))
                && !crate::addon::functions::value_matches(&value, &pin.data_type)
            {
                return Err(DaemonError::Execution("Addon function output type mismatch".into()));
            }
            let out_pin = caller
                .pins
                .iter()
                .find(|p| p.name == pin.name && p.pin_type == PinType::DataOutput)
                .ok_or_else(|| {
                    DaemonError::Execution(format!(
                        "CallFunction caller lacks output pin '{}'",
                        pin.name
                    ))
                })?;
            self.state.data_values.insert(out_pin.id, value.clone());
            caller_outputs.push((out_pin.id, value));
        }

        self.emit(ExecutionEvent::NodeFinished {
            node_id: caller_id,
        });
        self.emit(ExecutionEvent::NodeData {
            node_id: caller_id,
            outputs: caller_outputs,
            function: None,
        });
        ctx.audit(
            "node.finished",
            serde_json::json!({ "node_id": caller_id.to_string(), "kind": "CallFunction" }),
        );
        // Close the CallFunction tree node the caller opened at start.
        self.tree_end(ctx, TreeNodeStatus::Done);
        self.commit_successors(&outer_bp, caller_id, None, ctx)
    }
}
