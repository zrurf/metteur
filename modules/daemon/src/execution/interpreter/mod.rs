//! The blueprint interpreter.
//!
//! The interpreter is split by concern:
//! - [`lifecycle`]: run entry points, resume, and per-run setup.
//! - [`run`]: the worklist loop and terminal handling.
//! - [`edges`]: execution-edge and data-flow traversal of the active frame.
//! - [`frames`]: function frame management.
//! - [`foreach`]: ForEach loop driving.
//! - [`retry`]: validation retry and the circuit breaker.
//! - [`telemetry`]: execution-tree bookkeeping.
//! - [`checkpointing`]: checkpoint serialization.
//! - [`events`]: the event enum.

mod checkpointing;
mod circuit;
mod edges;
mod events;
mod foreach;
mod oversight;
mod frames;
mod lifecycle;
mod retry;
mod run;
mod telemetry;

pub use events::ExecutionEvent;

use std::sync::Arc;

use metteur_shared::config::Config;
use metteur_shared::llm::{SystemFragment, TodoItem};
use metteur_shared::{Blueprint, NodeId};
use parking_lot::RwLock as PLock;
use tokio::sync::RwLock;

use crate::execution::JobManager;
use crate::execution::checkpoint::CheckpointSink;
use crate::execution::context::{ExecutionState, RetryMark, Scheduler};
use crate::execution::transaction::TransactionLog;
use crate::execution::tree::ExecTree;
use crate::integration::lsp::LspManager;
use crate::llm::LlmClientFactory;
use crate::observability::audit::AuditWriter;
use crate::observability::metrics::Metrics;
use crate::registry::Registry;
use crate::sandbox::approval::ApprovalBroker;
use crate::storage::persistence::Db;
use crate::storage::versioning::VersionManager;

/// Shared handle to the executing root blueprint.
///
/// The ReplanBlueprint tool rewrites the plan through this handle while the
/// run reads a fresh snapshot at every node boundary (hot apply).
pub type SharedBlueprint = Arc<PLock<Blueprint>>;

/// Maximum nesting depth of function body frames.
const MAX_FUNCTION_DEPTH: u32 = 8;

/// Executes a blueprint against the given registry.
pub struct Interpreter {
    pub(crate) registry: Arc<Registry>,
    pub(crate) llm_factory: LlmClientFactory,
    pub(crate) workspace_root: std::path::PathBuf,
    pub(crate) state: ExecutionState,
    pub(crate) scheduler: Scheduler,
    pub(crate) events: Vec<ExecutionEvent>,
    pub(crate) view: crate::execution::view::ExecutionView,
    pub(crate) checkpoint: Option<Arc<dyn CheckpointSink>>,
    pub(crate) in_flight: Option<NodeId>,
    pub(crate) audit: Option<AuditWriter>,
    pub(crate) config: Option<Arc<RwLock<Config>>>,
    pub(crate) user: String,
    pub(crate) blueprint_id: uuid::Uuid,
    pub(crate) started_at: u64,
    /// Live event sink; when set, events stream out instead of buffering.
    pub(crate) event_tx: Option<tokio::sync::mpsc::UnboundedSender<ExecutionEvent>>,
    pub(crate) approvals: Option<Arc<ApprovalBroker>>,
    pub(crate) owns_approvals: bool,
    pub(crate) metrics: Option<Arc<Metrics>>,
    pub(crate) transaction_log: Option<TransactionLog>,
    pub(crate) workspace_db: Option<Db>,
    pub(crate) global_db: Option<Db>,
    pub(crate) lsp: Option<Arc<LspManager>>,
    pub(crate) lsp_source: Option<crate::integration::lsp::SharedLsp>,
    pub(crate) addon_fragments: Vec<SystemFragment>,
    pub(crate) version_manager: Option<Arc<VersionManager>>,
    pub(crate) jobs: Option<Arc<JobManager>>,
    pub(crate) shared_blueprint: Option<SharedBlueprint>,
    pub(crate) circuit_failures: u32,
    /// Whether a cancelled run rolls its file mutations back before exiting.
    ///
    /// Command side effects stay outside the WAL either way; this only governs
    /// the file mutations the run recorded.
    pub(crate) rollback_on_cancel: bool,
    pub(crate) tree: ExecTree,
    /// The run root of the tree, if the run started one.
    pub(crate) tree_root: Option<String>,
    /// Tree ids of entered function frames, innermost last.
    pub(crate) frame_trees: Vec<String>,
    /// Tree id of the node currently executing, if any.
    pub(crate) current_tree: Option<String>,
    /// Task list restored from a checkpoint, applied to the first context built
    /// for the resumed run (the list lives on the context, not the scheduler).
    pub(crate) resume_todos: Vec<TodoItem>,
    pub(crate) hook_cursor: crate::addon::hooks::Cursor,
}

impl Interpreter {
    /// Creates a new interpreter for the given registry.
    pub fn new(
        registry: Arc<Registry>,
        llm_factory: LlmClientFactory,
        workspace_root: std::path::PathBuf,
    ) -> Self {
        Self {
            registry,
            llm_factory,
            workspace_root,
            state: ExecutionState::default(),
            scheduler: Scheduler::default(),
            events: Vec::new(),
            view: Default::default(),
            checkpoint: None,
            in_flight: None,
            audit: None,
            config: None,
            user: "local".to_string(),
            blueprint_id: uuid::Uuid::nil(),
            started_at: 0,
            event_tx: None,
            approvals: None,
            owns_approvals: false,
            metrics: None,
            transaction_log: None,
            workspace_db: None,
            global_db: None,
            lsp: None,
            lsp_source: None,
            addon_fragments: Vec::new(),
            version_manager: None,
            jobs: None,
            shared_blueprint: None,
            circuit_failures: 0,
            rollback_on_cancel: true,
            tree: ExecTree::new(),
            tree_root: None,
            frame_trees: Vec::new(),
            current_tree: None,
            resume_todos: Vec::new(),
            hook_cursor: Default::default(),
        }
    }

    /// Enables checkpoint persistence through the given sink.
    pub fn with_checkpoint_sink(mut self, sink: Arc<dyn CheckpointSink>) -> Self {
        self.checkpoint = Some(sink);
        self
    }

    /// Attaches a workspace audit writer.
    pub fn with_audit(mut self, audit: AuditWriter) -> Self {
        self.audit = Some(audit);
        self
    }

    /// Attaches the merged workspace configuration.
    pub fn with_config(mut self, config: Arc<RwLock<Config>>) -> Self {
        self.config = Some(config);
        self
    }

    /// Attaches the authenticated subject performing the execution.
    pub fn with_user(mut self, user: impl Into<String>) -> Self {
        self.user = user.into();
        self
    }

    /// Streams execution events through `tx` as they occur instead of
    /// buffering them until the run completes.
    pub fn with_event_tx(mut self, tx: tokio::sync::mpsc::UnboundedSender<ExecutionEvent>) -> Self {
        self.event_tx = Some(tx);
        self
    }

    /// Attaches the sandbox approval broker shared with the control RPCs.
    /// Each execution requires a fresh broker; ending a run closes it.
    pub fn with_approvals(mut self, approvals: Arc<ApprovalBroker>) -> Self {
        self.approvals = Some(approvals);
        self.owns_approvals = true;
        self
    }

    /// Shares the outer run's broker without ending it when a subgraph returns.
    pub(crate) fn with_inherited_approvals(mut self, approvals: Arc<ApprovalBroker>) -> Self {
        self.approvals = Some(approvals);
        self.owns_approvals = false;
        self
    }

    /// Attaches process-wide metrics.
    pub fn with_metrics(mut self, metrics: Arc<Metrics>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Shares an existing transaction log so nested runs record into the
    /// parent log and outer rollbacks cover their mutations.
    pub fn with_transaction_log(mut self, log: TransactionLog) -> Self {
        self.transaction_log = Some(log);
        self
    }

    /// Attaches the workspace database for persistent sandbox grants.
    pub fn with_workspace_db(mut self, db: Db) -> Self {
        self.workspace_db = Some(db);
        self
    }

    /// Attaches the global database for global sandbox grants.
    pub fn with_global_db(mut self, db: Db) -> Self {
        self.global_db = Some(db);
        self
    }

    /// Shares reloads with existing contexts, including nested interpreters.
    pub fn with_lsp_source(mut self, source: crate::integration::lsp::SharedLsp) -> Self {
        self.lsp_source = Some(source);
        self
    }

    /// Attaches the workspace language-server manager.
    pub fn with_lsp(mut self, lsp: Arc<LspManager>) -> Self {
        self.lsp = Some(lsp);
        self
    }

    /// Attaches addon prompt fragments for fresh CallLLM contexts.
    pub fn with_addon_fragments(mut self, fragments: Vec<SystemFragment>) -> Self {
        self.addon_fragments = fragments;
        self
    }

    /// Attaches the workspace version manager, enabling snapshot tools.
    pub fn with_version_manager(mut self, version_manager: Arc<VersionManager>) -> Self {
        self.version_manager = Some(version_manager);
        self
    }

    /// Shares the workspace's background-command manager with the run.
    ///
    /// Job ids stay valid across runs of the workspace, and the manager can be
    /// asked to clean up everything a run started.
    pub fn with_jobs(mut self, jobs: Arc<JobManager>) -> Self {
        self.jobs = Some(jobs);
        self
    }

    /// Emits an event to the live sink or buffers it when no sink is set.
    pub(crate) fn emit(&mut self, event: ExecutionEvent) {
        let scope = self
            .active_function()
            .and_then(|id| self.registry.function_by_id(id))
            .map(|f| f.body.id)
            .unwrap_or(self.blueprint_id)
            .to_string();
        self.view.observe(&event, &scope, checkpointing::now_millis());
        match &self.event_tx {
            Some(tx) => {
                let _ = tx.send(event);
            }
            None => self.events.push(event),
        }
    }

    /// Returns the retry attempt count recorded for `node_id`.
    pub fn attempt_count(&self, node_id: NodeId) -> Option<u32> {
        self.state.attempt_counts.get(&node_id).copied()
    }

    /// Returns the retry rollback mark recorded for `node_id`.
    pub fn validation_mark(&self, node_id: NodeId) -> Option<RetryMark> {
        self.state.validation_marks.get(&node_id).copied()
    }

    /// Returns the completion order of the active frame's scheduler.
    pub fn execution_order(&self) -> Vec<NodeId> {
        self.active_scheduler().order_list()
    }

    /// Returns the run's execution tree.
    pub fn exec_tree(&self) -> &ExecTree {
        &self.tree
    }
}
