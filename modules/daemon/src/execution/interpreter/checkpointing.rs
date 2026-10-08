//! Checkpoint serialization for the running interpreter.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use metteur_shared::{Blueprint, NodeId};

use crate::error::{DaemonError, DaemonResult};
use crate::execution::checkpoint::{
    CHECKPOINT_TRANSITION_VERSION, CheckpointSink, ExecutionCheckpoint, RunStatus,
};
use crate::execution::context::ExecutionContext;

use super::Interpreter;

impl Interpreter {
    /// Commits a completed transition only after its continuation is queued.
    /// Frame, variable, loop, retry and tree updates must precede this call.
    /// `pin_name` selects a ForEach branch; other nodes dispatch all eligible
    /// execution edges and wake their data consumers.
    pub(crate) fn commit_successors(
        &mut self,
        blueprint: &Blueprint,
        node_id: NodeId,
        pin_name: Option<&str>,
        ctx: &ExecutionContext,
    ) -> DaemonResult<()> {
        match pin_name {
            Some(name) => self.fire_named_edge(blueprint, node_id, name)?,
            None => self.fire_edges(blueprint, node_id)?,
        }
        self.write_checkpoint(ctx)
    }

    /// Persists a checkpoint of the current run state, if a sink is set.
    pub(crate) fn write_checkpoint(&mut self, ctx: &ExecutionContext) -> DaemonResult<()> {
        let Some(sink) = self.checkpoint.clone() else {
            return Ok(());
        };
        self.persist(&sink, ctx, RunStatus::Running, None)
    }

    /// Persists the terminal checkpoint for a finished run.
    pub(crate) fn write_terminal_checkpoint(
        &mut self,
        ctx: &ExecutionContext,
        status: RunStatus,
        error: Option<String>,
    ) -> DaemonResult<()> {
        let Some(sink) = self.checkpoint.clone() else {
            return Ok(());
        };
        self.persist(&sink, ctx, status, error)
    }

    /// Serializes and writes a checkpoint through the sink.
    fn persist(
        &mut self,
        sink: &Arc<dyn CheckpointSink>,
        ctx: &ExecutionContext,
        status: RunStatus,
        error: Option<String>,
    ) -> DaemonResult<()> {
        self.view.root = self.shared_blueprint.as_ref().map(|bp| bp.read().clone());
        let mut checkpoint = ExecutionCheckpoint {
            view: self.view.clone(),
            transition_version: CHECKPOINT_TRANSITION_VERSION,
            addon_identity_version: 1,
            addon_packages: ctx.registry.addon_packages.clone(),
            function_identities: Some(ctx.registry.function_identities()),
            in_flight: self.in_flight,
            run_id: sink.run_id(),
            blueprint_id: self.blueprint_id,
            blueprint_version: None,
            status,
            started_at: self.started_at,
            updated_at: now_millis(),
            call_stack: self.state.call_stack.clone(),
            data_values: self.state.data_values.clone(),
            executed: self.scheduler.executed_list(),
            pending: self.scheduler.pending_list(),
            triggered: self.scheduler.triggered_list(),
            transaction_log: ctx.transaction_log.entries().to_vec(),
            executed_order: self.scheduler.order_list(),
            attempt_counts: self.state.attempt_counts.clone(),
            validation_marks: self.state.validation_marks.clone(),
            variables: ctx.variables.clone(),
            foreach_stack: self.state.foreach_stack.clone(),
            todos: ctx.todos.clone(),
            exec_tree: self.tree.clone(),
            frame_trees: self.frame_trees.clone(),
            current_tree: self.current_tree.clone(),
            circuit_failures: self.circuit_failures,
            error,
        };
        crate::replan::application::commit_boundary(ctx, &mut checkpoint, sink.as_ref()).map_err(|err| {
            let message = format!(
                "run {} checkpoint failed: {err}; no new effects will start; uncommitted external outcomes require manual recovery",
                sink.run_id(),
            );
            self.emit(super::ExecutionEvent::Message { node_id: ctx.current_node, message: message.clone() });
            DaemonError::Persistence(message)
        })?;
        // A successful apply may add validity changes at the same boundary.
        self.view = checkpoint.view;
        self.hook_cursor.committed(&ctx.registry.addon_hooks, &self.view, &ctx.workspace_root, sink.run_id(), checkpoint.status);
        Ok(())
    }
}

/// Returns the current time in milliseconds since the Unix epoch.
pub(crate) fn now_millis() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}
