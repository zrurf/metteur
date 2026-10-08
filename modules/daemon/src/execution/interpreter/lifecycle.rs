//! Run entry points, resume, and per-run setup.

use std::collections::HashMap;
use std::sync::Arc;

use crate::error::{DaemonError, DaemonResult};
use crate::execution::checkpoint::{CHECKPOINT_TRANSITION_VERSION, ExecutionCheckpoint};
use crate::execution::context::{ExecutionContext, ExecutionState, Frame, Scheduler};
use crate::execution::interrupt::InterruptBus;
use crate::execution::transaction::TransactionLog;
use crate::execution::tree::{ExecTree, TreeNodeKind};

use super::checkpointing::now_millis;
use super::edges::has_exec_cycle;
use super::{Interpreter, SharedBlueprint};

#[cfg(test)]
#[path = "paused_apply_tests.rs"]
mod paused_apply_tests;

impl Interpreter {
    /// Executes the blueprint from its entry node.
    ///
    /// The interpreter state is reset on each call, so an instance may be
    /// reused across executions. `interrupts` optionally provides a channel
    /// for injecting temporary messages during execution.
    pub async fn run(
        &mut self,
        blueprint: &SharedBlueprint,
        interrupts: Option<InterruptBus>,
    ) -> DaemonResult<Vec<super::ExecutionEvent>> {
        self.run_with_control(
            blueprint,
            interrupts,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .await
    }

    /// Executes the blueprint with shared pause/cancel control flags.
    ///
    /// The flags are shared with the gRPC control RPCs so that pause and
    /// cancel requests take effect during execution. `blueprint` is shared so
    /// a replan can hot-apply data-level edits to the remainder of the run.
    pub async fn run_with_control(
        &mut self,
        blueprint: &SharedBlueprint,
        interrupts: Option<InterruptBus>,
        pause_requested: Arc<std::sync::atomic::AtomicBool>,
        cancel_requested: Arc<std::sync::atomic::AtomicBool>,
    ) -> DaemonResult<Vec<super::ExecutionEvent>> {
        let _approval_lifetime = self
            .approvals
            .as_ref()
            .filter(|_| self.owns_approvals)
            .map(|broker| broker.close_on_drop());
        // Reject blueprints whose execution graph contains a cycle.
        self.registry.validate_addon_nodes(&blueprint.read())?;
        if has_exec_cycle(&blueprint.read()) {
            return Err(DaemonError::Execution(
                "execution cycle detected in blueprint".to_string(),
            ));
        }

        self.shared_blueprint = Some(blueprint.clone());
        self.reset_run(blueprint);
        let mut ctx = self.make_context(interrupts, pause_requested, cancel_requested).await;
        crate::replan::application::attach(&ctx, None)?;
        self.write_checkpoint(&ctx)?;
        let _oversight = crate::oversight::runtime::start(&ctx).await;
        self.execute(blueprint, &mut ctx).await
    }

    /// Resumes an interrupted run from its checkpoint.
    ///
    /// The scheduling state, produced data values and transaction log are
    /// restored and execution continues with checkpoints written to the same
    /// run id. Uncommitted nodes and mismatched file phases require manual
    /// recovery; unknown side effects are never automatically replayed.
    pub async fn resume_with_control(
        &mut self,
        blueprint: &SharedBlueprint,
        resume: ExecutionCheckpoint,
        interrupts: Option<InterruptBus>,
        pause_requested: Arc<std::sync::atomic::AtomicBool>,
        cancel_requested: Arc<std::sync::atomic::AtomicBool>,
    ) -> DaemonResult<Vec<super::ExecutionEvent>> {
        let _approval_lifetime = self
            .approvals
            .as_ref()
            .filter(|_| self.owns_approvals)
            .map(|broker| broker.close_on_drop());
        resume.ensure_addons(&self.registry)?;
        self.registry.validate_addon_nodes(&blueprint.read())?;
        if !resume.status.resumable() {
            return Err(DaemonError::Interrupted(format!(
                "run {} is not resumable",
                resume.run_id
            )));
        }
        // Old writers saved some branches before dispatching successors. Even
        // a nonempty queue cannot prove that a parallel continuation was not
        // lost, so do not upgrade such records by guessing their next step.
        if resume.transition_version != CHECKPOINT_TRANSITION_VERSION {
            return Err(DaemonError::Execution(format!(
                "run {} has unsupported checkpoint transition version {}; successor dispatch cannot be verified, manual recovery required",
                resume.run_id, resume.transition_version
            )));
        }
        if resume.blueprint_id != blueprint.read().id {
            return Err(DaemonError::Execution(
                "checkpoint belongs to a different blueprint".to_string(),
            ));
        }
        if let Some(db) = &self.workspace_db {
            crate::oversight::recovery::ensure_resumable(db, resume.run_id)?;
        }
        if let Some(node_id) = resume.in_flight {
            return Err(DaemonError::Persistence(format!(
                "run {} has an uncommitted outcome for node {node_id}; manual recovery required, automatic replay refused",
                resume.run_id
            )));
        }
        // Verify version identity before moving fields out of the checkpoint.
        self.shared_blueprint = Some(blueprint.clone());
        let probe =
            self.make_context(None, pause_requested.clone(), cancel_requested.clone()).await;
        crate::replan::application::attach(&probe, Some(&resume))?;
        self.in_flight = None;
        self.shared_blueprint = Some(blueprint.clone());
        self.state = ExecutionState {
            blueprint_id: resume.blueprint_id,
            call_stack: resume.call_stack,
            data_values: resume.data_values,
            attempt_counts: resume.attempt_counts,
            validation_marks: resume.validation_marks,
            foreach_stack: resume.foreach_stack.clone(),
        };
        self.scheduler = Scheduler::from_checkpoint(
            resume.executed,
            resume.pending,
            resume.triggered,
            resume.executed_order,
        );
        self.events.clear();
        self.view = Default::default();
        self.blueprint_id = resume.blueprint_id;
        self.started_at = resume.started_at;
        // The task list lives on the execution context (not the scheduler
        // state), so it is carried into the resumed run separately.
        self.resume_todos = resume.todos.clone();
        // Carried over so a resumed run cannot reset the breaker counter and
        // slip past a replan threshold it had already reached.
        self.circuit_failures = resume.circuit_failures;
        self.view = resume.view.clone();
        self.hook_cursor.seed(&self.view);
        self.tree = resume.exec_tree.clone();
        self.tree_root = resume.exec_tree.roots.first().cloned();
        self.frame_trees = resume.frame_trees.clone();
        self.current_tree = resume.current_tree.clone();

        let mut ctx = self.make_context(interrupts, pause_requested, cancel_requested).await;
        ctx.blueprint_apply = probe.blueprint_apply;
        ctx.transaction_log = TransactionLog::from_entries(resume.transaction_log);
        ctx.attach_file_journal();
        ctx.transaction_log.validate_resume(resume.run_id)?;
        // Checkpoints predating frame variables resume with an empty stack;
        // restore the root frame so VariableSet has somewhere to write.
        ctx.variables = if resume.variables.is_empty() {
            vec![HashMap::new()]
        } else {
            resume.variables
        };
        self.write_checkpoint(&ctx)?;
        let _oversight = crate::oversight::runtime::start(&ctx).await;
        self.execute(blueprint, &mut ctx).await
    }

    /// Resets the interpreter for a fresh run of `blueprint`.
    fn reset_run(&mut self, blueprint: &SharedBlueprint) {
        self.hook_cursor = Default::default();
        let bp = blueprint.read();
        self.state = ExecutionState::default();
        self.in_flight = None;
        self.state.blueprint_id = bp.id;
        self.state.call_stack.push(Frame {
            node_id: bp.entry_node_id,
            pc: 0,
            function: None,
        });
        self.events.clear();
        self.view = Default::default();
        self.scheduler = Scheduler::default();
        self.scheduler.seed(bp.entry_node_id);
        self.blueprint_id = bp.id;
        self.started_at = now_millis();
        self.circuit_failures = 0;
        self.tree = ExecTree::new();
        let root = self.tree.spawn(None, TreeNodeKind::Run, bp.name.clone(), self.started_at);
        self.tree_root = Some(root);
        self.frame_trees.clear();
        self.current_tree = None;
        // A fresh run starts with an empty task list; a stale one would leak
        // into `make_context` (which seeds the list from this field).
        self.resume_todos.clear();
    }

    /// Creates the per-run execution context.
    async fn make_context(
        &self,
        interrupts: Option<InterruptBus>,
        pause_requested: Arc<std::sync::atomic::AtomicBool>,
        cancel_requested: Arc<std::sync::atomic::AtomicBool>,
    ) -> ExecutionContext {
        let run_id = self.checkpoint.as_ref().map(|sink| sink.run_id()).unwrap_or_default();
        let mut ctx = ExecutionContext::new(
            self.registry.clone(),
            self.llm_factory.clone(),
            self.workspace_root.clone(),
        )
        .with_run(run_id, self.started_at);
        if let Some(audit) = &self.audit {
            ctx.audit = Some(audit.clone());
        }
        if let Some(config) = &self.config {
            let mode =
                metteur_shared::config::effective_permission_mode(&config.read().await.sandbox);
            ctx.permission_mode = crate::sandbox::PermissionMode::parse(mode);
            ctx.config = Some(config.clone());
        }
        ctx.user = self.user.clone();
        ctx.interrupts = interrupts;
        ctx.pause_requested = pause_requested;
        ctx.cancel_requested = cancel_requested;
        ctx.events = self.event_tx.clone();
        ctx.approvals = self.approvals.clone();
        ctx.metrics = self.metrics.clone();
        if let Some(log) = &self.transaction_log {
            ctx.transaction_log = log.clone();
        }
        ctx.workspace_db = self.workspace_db.clone();
        ctx.global_db = self.global_db.clone();
        ctx.lsp = self.lsp.clone();
        ctx.lsp_source = self.lsp_source.clone();
        ctx.addon_fragments = self.addon_fragments.clone();
        ctx.blueprint = self.shared_blueprint.clone();
        ctx.version_manager = self.version_manager.clone();
        if let Some(jobs) = &self.jobs {
            ctx.jobs = Arc::clone(jobs);
        }
        ctx.todos = self.resume_todos.clone();
        ctx.attach_file_journal();
        ctx
    }
}
