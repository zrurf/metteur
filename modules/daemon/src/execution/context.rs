//! Execution state and shared execution context.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use metteur_shared::config::Config;
use metteur_shared::{Blueprint, NodeId, PinId, Value};
use tokio::sync::RwLock;

use crate::llm::LlmClientFactory;
use crate::observability::audit::AuditWriter;
use crate::registry::Registry;
use crate::storage::persistence::Db;

use super::interrupt::InterruptBus;
use super::transaction::TransactionLog;

/// A frame on the execution call stack.
///
/// The bottom frame tracks the in-flight root node; every frame above it
/// represents an entered function body with its own scheduler.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Frame {
    /// The node currently associated with this frame: for the root frame the
    /// in-flight root node, for a function frame the caller's `CallFunction`
    /// node.
    pub node_id: NodeId,
    /// The index of the next output execution pin to follow.
    pub pc: usize,
    /// Present while executing inside a function body.
    #[serde(default)]
    pub function: Option<FunctionBody>,
}

/// Runtime state of one called function body.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FunctionBody {
    /// The function entry id; the body is re-fetched from the registry on
    /// resume.
    pub id: uuid::Uuid,
    /// The independent scheduler of this function frame.
    pub scheduler: Scheduler,
}

/// Scheduling state of one execution frame.
///
/// The root frame's scheduler lives in [`ExecutionState`]; each entered
/// function body carries its own so nested bodies schedule independently.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Scheduler {
    triggered: HashSet<NodeId>,
    executed: HashSet<NodeId>,
    queue: VecDeque<NodeId>,
    /// Completed node ids in completion order; defines the retry segment
    /// boundary used by validation rollback.
    #[serde(default)]
    order: Vec<NodeId>,
}

impl Scheduler {
    /// Reconstructs a scheduler from checkpoint lists.
    pub fn from_checkpoint(
        executed: Vec<NodeId>,
        pending: Vec<NodeId>,
        triggered: Vec<NodeId>,
        order: Vec<NodeId>,
    ) -> Self {
        Self {
            triggered: triggered.into_iter().collect(),
            executed: executed.into_iter().collect(),
            queue: pending.into_iter().collect(),
            order,
        }
    }

    /// Seeds the worklist with the frame's entry node.
    pub fn seed(&mut self, entry: NodeId) {
        self.triggered.insert(entry);
        self.queue.push_back(entry);
    }

    /// Completed node ids, in arbitrary order.
    pub fn executed_list(&self) -> Vec<NodeId> {
        self.executed.iter().copied().collect()
    }

    /// All completed node ids in completion order.
    pub fn order_list(&self) -> Vec<NodeId> {
        self.order.clone()
    }

    /// Pending (queued) node ids, in scheduling order.
    pub fn pending_list(&self) -> Vec<NodeId> {
        self.queue.iter().copied().collect()
    }

    /// Ever-triggered node ids, in arbitrary order.
    pub fn triggered_list(&self) -> Vec<NodeId> {
        self.triggered.iter().copied().collect()
    }

    /// Whether `node_id` already completed.
    pub fn is_executed(&self, node_id: NodeId) -> bool {
        self.executed.contains(&node_id)
    }

    /// Whether `node_id` was ever triggered.
    pub fn is_triggered(&self, node_id: NodeId) -> bool {
        self.triggered.contains(&node_id)
    }

    /// Marks a node as completed, recording its position in the execution
    /// order.
    pub fn mark_executed(&mut self, node_id: NodeId) {
        if self.executed.insert(node_id) {
            self.order.push(node_id);
        }
    }

    /// Un-marks a node so a retry or replan can re-queue it.
    pub fn unmark_executed(&mut self, node_id: NodeId) {
        self.executed.remove(&node_id);
        self.order.retain(|n| *n != node_id);
    }

    /// Current length of the completion order, usable as a segment mark.
    pub fn order_len(&self) -> usize {
        self.order.len()
    }

    /// Completed node ids from `start` onwards, in completion order.
    pub fn order_from(&self, start: usize) -> Vec<NodeId> {
        self.order.iter().skip(start).copied().collect()
    }

    /// Removes the next queued node, if any.
    pub fn pop(&mut self) -> Option<NodeId> {
        self.queue.pop_front()
    }

    /// Removes all queued occurrences of `ids`.
    ///
    /// A node fed by both an execution edge and a data edge is enqueued twice
    /// (the second pop is normally skipped as already executed). Retry and
    /// replan paths un-mark segment nodes, which would resurrect the stale
    /// duplicate, so they drain the queue first and re-queue each node once.
    pub fn dequeue_all(&mut self, ids: &[NodeId]) {
        if ids.is_empty() {
            return;
        }
        self.queue.retain(|n| !ids.contains(n));
    }

    /// Marks `node_id` as triggered and queues it when inputs allow later.
    pub fn enqueue(&mut self, node_id: NodeId) {
        self.triggered.insert(node_id);
        self.queue.push_back(node_id);
    }
}

/// Rollback/retry boundary captured when a validator last passed.
///
/// Marks index into the transaction log (which mutations to undo) and into
/// the active frame's completion order (which nodes to re-execute).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RetryMark {
    /// Transaction log length at the last pass.
    pub log_mark: usize,
    /// Completion-order length at the last pass.
    pub order_mark: usize,
}

/// The runtime state of a single blueprint execution.
#[derive(Debug, Default)]
#[allow(dead_code)]
pub struct ExecutionState {
    /// The blueprint being executed.
    pub blueprint_id: uuid::Uuid,
    /// The execution call stack (root frame + function frames).
    pub call_stack: Vec<Frame>,
    /// Values produced on data output pins.
    pub data_values: HashMap<PinId, Value>,
    /// Failed validation attempts per validator node.
    pub attempt_counts: HashMap<NodeId, u32>,
    /// Rollback boundaries per validator node, refreshed on each pass.
    pub validation_marks: HashMap<NodeId, RetryMark>,
    /// Active ForEach loops, innermost last (one per frame at most).
    pub foreach_stack: Vec<ForEachState>,
}

/// The runtime state of one active ForEach loop.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ForEachState {
    /// The ForEach node owning the loop.
    pub node_id: NodeId,
    /// The iterated items.
    pub items: Vec<Value>,
    /// Index of the item currently (or next) exposed on `Iteration`.
    pub index: usize,
    /// Call-stack depth of the owning frame; loops never cross frames.
    pub depth: usize,
    /// Body entries so far, bounded by `foreach_max_iterations`.
    pub count: u32,
}

/// A context mutation requested by a tool and applied by the ReAct loop.
///
/// Tools cannot reach the conversation context directly — it lives in the loop
/// that owns the turn. A tool that changes the context queues an operation
/// here, and the loop drains the queue after the tool batch and before the next
/// model call, so a release never targets the result that requested it.
#[derive(Debug, Clone)]
pub enum ContextOp {
    /// Release the tool results selected by the query.
    Release(metteur_shared::llm::ReleaseQuery),
}

/// Shared capabilities available to node executors during execution.
///
/// This is created once per execution and passed by mutable reference to each
/// node executor, giving them access to the registry, LLM clients, audit
/// logging, interrupt handling and the transaction log.
pub struct ExecutionContext {
    /// The resource registry.
    pub registry: Arc<Registry>,
    /// The LLM client factory.
    pub llm_factory: LlmClientFactory,
    /// The workspace root path.
    pub workspace_root: std::path::PathBuf,
    /// The transaction log for rollback.
    pub transaction_log: TransactionLog,
    /// The interrupt bus, if interrupts are enabled.
    pub interrupts: Option<InterruptBus>,
    /// Normal interrupts deferred until the next Call LLM node.
    pub pending_normal: VecDeque<String>,
    /// Set when execution should pause (shared with the control RPCs).
    pub pause_requested: Arc<std::sync::atomic::AtomicBool>,
    /// Set when execution should cancel (shared with the control RPCs).
    pub cancel_requested: Arc<std::sync::atomic::AtomicBool>,
    /// The workspace audit writer, if audit logging is enabled.
    pub audit: Option<AuditWriter>,
    /// The id of the current execution run.
    pub run_id: uuid::Uuid,
    /// Execution start time in milliseconds since the Unix epoch.
    pub started_at: u64,
    /// The authenticated subject performing the execution.
    pub user: String,
    /// The merged workspace configuration (for LLM defaults and reloads).
    pub config: Option<Arc<RwLock<Config>>>,
    /// The node currently being executed.
    pub current_node: NodeId,
    /// Execution attempt associated with durable file operations.
    pub file_attempt: u32,
    /// Sandbox approval channel, when live event streaming is attached.
    pub approvals: Option<Arc<crate::sandbox::approval::ApprovalBroker>>,
    /// Live event sink for streaming events to clients as they occur.
    pub events: Option<tokio::sync::mpsc::UnboundedSender<super::interpreter::ExecutionEvent>>,
    /// The workspace database (persistent sandbox grants).
    pub workspace_db: Option<Db>,
    /// The global database (global grants).
    pub global_db: Option<Db>,
    /// SubAgent nesting depth of this context.
    pub depth: u32,
    /// Process-wide metrics, when attached.
    pub metrics: Option<Arc<crate::observability::metrics::Metrics>>,
    /// Language-server manager for this workspace, when LSP is enabled.
    pub lsp: Option<Arc<crate::integration::lsp::LspManager>>,
    /// Live workspace slot, preferred over the standalone manager.
    pub lsp_source: Option<crate::integration::lsp::SharedLsp>,
    /// Addon prompt fragments injected into fresh CallLLM contexts.
    pub addon_fragments: Vec<metteur_shared::llm::SystemFragment>,
    /// Shared handle to the executing root blueprint (replan hot-apply).
    pub blueprint: Option<Arc<parking_lot::RwLock<Blueprint>>>,
    pub(crate) blueprint_apply: Arc<parking_lot::Mutex<crate::replan::application::ApplyState>>,
    /// Frame-scoped variable maps, innermost last. The interpreter pushes one
    /// per entered function frame; executors read and write through it.
    pub variables: Vec<HashMap<String, Value>>,
    /// Tree operations queued by executors, drained by the interpreter when
    /// the node finishes.
    pub tree_ops: Vec<super::tree::TreeOp>,
    /// The workspace version manager, enabling snapshot tools.
    pub version_manager: Option<Arc<crate::storage::versioning::VersionManager>>,
    /// Background commands of this workspace.
    ///
    /// A fresh manager is created per context so tools always work; the daemon
    /// swaps in the workspace-wide instance when it spawns a run or a chat, so
    /// job ids stay valid across runs and can be listed per workspace.
    pub jobs: Arc<super::jobs::JobManager>,
    /// Paths read by tools since the last drain; the ReAct loop records them
    /// on the tool result so the context manager can expire stale content.
    pub read_paths: Vec<std::path::PathBuf>,
    /// Paths *fully* read by tools since the last drain (a windowed read is
    /// excluded); the ReAct loop supersedes earlier full reads of the same
    /// paths.
    pub read_paths_full: Vec<std::path::PathBuf>,
    /// Context mutations requested by tools, drained by the ReAct loop.
    pub context_ops: Vec<ContextOp>,
    /// Who answers authorization questions during this run.
    ///
    /// Set from the workspace configuration when the run starts and overridden
    /// per conversation by the client's permission selector.
    pub permission_mode: crate::sandbox::PermissionMode,
    /// Progress sink of the tool call that is currently running.
    ///
    /// A long tool reports trailing output here (a build's last lines) and the
    /// ReAct loop forwards it as an event while the call is still in flight.
    /// `None` outside a call, and outside the streaming ReAct variant.
    pub progress: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    /// Whether a ReAct loop currently drives this context.
    ///
    /// A tool that only queues a context operation (`ReleaseContext`) has no
    /// effect outside such a loop, because a blueprint pin value has no single
    /// conversation to mutate; the tool refuses instead of promising something
    /// it cannot deliver.
    pub in_react_loop: bool,
    /// The caller's conversation, present only while a tool that declares
    /// interest in it (`SpawnSubAgent`) is running.
    pub parent_context: Option<metteur_shared::llm::ContextManager>,
    /// Paths mutated by tools since the last drain; the ReAct loop drains them
    /// to invalidate earlier reads of the same files.
    pub mutated_paths: Vec<std::path::PathBuf>,
    /// The agent's task list for this context.
    ///
    /// A nested context (SubAgent) inherits a copy: its own plan must not
    /// rewrite the parent's.
    pub todos: Vec<metteur_shared::llm::TodoItem>,
}

impl ExecutionContext {
    /// Reuses the workspace database for durable tool file operations.
    pub fn attach_file_journal(&self) {
        self.transaction_log.bind_run(self.run_id);
        if let Some(db) = &self.workspace_db {
            self.transaction_log.attach_journal(super::file_journal::FileJournal::new(
                self.workspace_root.clone(),
                std::sync::Arc::new(super::file_journal::DbFileJournal(db.clone())),
            ));
        }
    }

    /// Writes after the caller has completed path and sandbox authorization.
    pub fn write_file(
        &mut self,
        path: &std::path::Path,
        bytes: &[u8],
        expected: Option<&[u8]>,
    ) -> crate::error::DaemonResult<()> {
        self.attach_file_journal();
        self.transaction_log.mutate_file(
            &self.workspace_root,
            path,
            Some(bytes),
            super::file_journal::FileOrigin {
                run_id: self.run_id,
                node_id: self.current_node,
                attempt: self.file_attempt,
                wal_position: 0,
            },
            expected,
        )?;
        self.note_file_mutation(path);
        Ok(())
    }
    /// Creates a new execution context.
    pub fn new(
        registry: Arc<Registry>,
        llm_factory: LlmClientFactory,
        workspace_root: std::path::PathBuf,
    ) -> Self {
        let jobs = Arc::new(super::jobs::JobManager::new(workspace_root.clone()));
        Self {
            registry,
            llm_factory,
            workspace_root,
            transaction_log: TransactionLog::new(),
            interrupts: None,
            pending_normal: VecDeque::new(),
            pause_requested: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            cancel_requested: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            audit: None,
            run_id: uuid::Uuid::nil(),
            started_at: 0,
            user: "local".to_string(),
            config: None,
            current_node: uuid::Uuid::nil(),
            file_attempt: 1,
            approvals: None,
            events: None,
            workspace_db: None,
            global_db: None,
            depth: 0,
            metrics: None,
            lsp: None,
            lsp_source: None,
            addon_fragments: Vec::new(),
            blueprint: None,
            blueprint_apply: Arc::new(parking_lot::Mutex::new(Default::default())),
            variables: vec![HashMap::new()],
            tree_ops: Vec::new(),
            version_manager: None,
            jobs,
            read_paths: Vec::new(),
            read_paths_full: Vec::new(),
            context_ops: Vec::new(),
            permission_mode: crate::sandbox::PermissionMode::default(),
            progress: None,
            in_react_loop: false,
            parent_context: None,
            mutated_paths: Vec::new(),
            todos: Vec::new(),
        }
    }

    /// Declares that the running tool read these workspace paths.
    ///
    /// Paths are recorded relative to the workspace root so the context
    /// manager can match them against later mutations.
    pub fn note_read_paths(&mut self, paths: impl IntoIterator<Item = std::path::PathBuf>) {
        for path in paths {
            self.read_paths.push(self.relative_path(path));
        }
    }

    /// Declares that the running tool mutated this workspace path.
    pub fn note_file_mutation(&mut self, path: impl AsRef<std::path::Path>) {
        self.mutated_paths.push(self.relative_path(path.as_ref().to_path_buf()));
    }

    /// Declares that the running tool read these paths in full.
    ///
    /// A full read supersedes an earlier full read of the same paths; a
    /// windowed read only re-registers the path for staleness tracking.
    pub fn note_full_read(&mut self, paths: impl IntoIterator<Item = std::path::PathBuf>) {
        for path in paths {
            let relative = self.relative_path(path);
            self.read_paths.push(relative.clone());
            self.read_paths_full.push(relative);
        }
    }

    /// Normalizes a path to a workspace-relative form (falls back to the
    /// input when it lies outside the workspace).
    fn relative_path(&self, path: std::path::PathBuf) -> std::path::PathBuf {
        let root = &self.workspace_root;
        path.strip_prefix(root).map(|p| p.to_path_buf()).unwrap_or(path)
    }

    /// Returns the innermost variable frame for writing, if any.
    pub fn variables_last_mut(&mut self) -> Option<&mut HashMap<String, Value>> {
        self.variables.last_mut()
    }

    /// Iterates variable frames from the innermost outwards for lookup.
    pub fn variables_frames(&self) -> impl DoubleEndedIterator<Item = &HashMap<String, Value>> {
        self.variables.iter()
    }

    /// Resolves the current workspace manager for each LSP operation.
    pub fn lsp_manager(&self) -> Option<Arc<crate::integration::lsp::LspManager>> {
        let user = match &self.lsp_source {
            Some(source) => source.read().clone(),
            None => self.lsp.clone(),
        };
        crate::integration::lsp::LspManager::combine(user, &self.registry.addon_lsp)
    }

    /// Attaches a standalone manager when there is no live workspace slot.
    pub fn with_lsp(mut self, lsp: Arc<crate::integration::lsp::LspManager>) -> Self {
        self.lsp = Some(lsp);
        self
    }

    /// Attaches an audit writer.
    pub fn with_audit(mut self, audit: AuditWriter) -> Self {
        self.audit = Some(audit);
        self
    }

    /// Creates a child context for nested execution (SubAgent / Abstract).
    ///
    /// The child shares the registry, LLM factory, workspace root,
    /// configuration, interrupt bus, approval broker, event sink, control
    /// flags, identity and audit writer of this context, but starts with a
    /// shared transaction log and an incremented nesting depth.
    pub fn child_nested(&self) -> Self {
        let mut child =
            Self::new(self.registry.clone(), self.llm_factory.clone(), self.workspace_root.clone());
        child.config = self.config.clone();
        child.interrupts = self.interrupts.clone();
        child.approvals = self.approvals.clone();
        child.events = self.events.clone();
        child.pause_requested = self.pause_requested.clone();
        child.cancel_requested = self.cancel_requested.clone();
        child.user = self.user.clone();
        child.audit = self.audit.clone();
        child.run_id = self.run_id;
        child.started_at = self.started_at;
        child.current_node = self.current_node;
        child.transaction_log = self.transaction_log.clone();
        child.file_attempt = self.file_attempt;
        child.workspace_db = self.workspace_db.clone();
        child.global_db = self.global_db.clone();
        child.metrics = self.metrics.clone();
        child.lsp = self.lsp.clone();
        child.lsp_source = self.lsp_source.clone();
        child.addon_fragments = self.addon_fragments.clone();
        child.blueprint = self.blueprint.clone();
        child.blueprint_apply = self.blueprint_apply.clone();
        child.version_manager = self.version_manager.clone();
        // The child works from the parent's plan but keeps its own copy, so a
        // sub-agent's updates cannot rewrite the caller's list.
        child.todos = self.todos.clone();
        child.depth = self.depth + 1;
        child
    }

    /// Attaches the current run identity.
    pub fn with_run(mut self, run_id: uuid::Uuid, started_at: u64) -> Self {
        self.run_id = run_id;
        self.started_at = started_at;
        self
    }

    /// Attaches the authenticated subject.
    pub fn with_user(mut self, user: impl Into<String>) -> Self {
        self.user = user.into();
        self
    }

    /// Attaches the merged workspace configuration.
    pub fn with_config(mut self, config: Arc<RwLock<Config>>) -> Self {
        self.config = Some(config);
        self
    }

    /// Records an audit entry if an audit writer is attached.
    pub fn audit(&self, operation: &str, detail: serde_json::Value) {
        if let Some(writer) = &self.audit {
            let _ = writer.record(&self.user, operation, detail);
        }
    }
}
