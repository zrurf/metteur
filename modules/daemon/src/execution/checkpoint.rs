//! Execution checkpoints for crash-safe resume.
//!
//! A checkpoint serializes the interpreter's scheduling state (executed and
//! pending nodes, produced data values, the transaction log) after each node.
//! On restart, an interrupted run can be resumed from its latest checkpoint.

use std::collections::HashMap;

use metteur_shared::{NodeId, PinId, Value};
use serde::{Deserialize, Serialize};

use crate::error::{DaemonError, DaemonResult};
use crate::storage::persistence::{Db, cf};

use super::context::Frame;
use super::transaction::TransactionEntry;

/// Checkpoints written after successor dispatch and frame/loop bookkeeping.
pub const CHECKPOINT_TRANSITION_VERSION: u32 = 2;

/// The lifecycle status of an execution run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunStatus {
    /// Actively executing.
    Running,
    /// Saved but not currently executing (e.g. after a crash).
    Suspended,
    /// Finished successfully.
    Completed,
    /// Stopped on an explicit user cancel; not an error and not resumable.
    ///
    /// Kept distinct from [`RunStatus::Failed`] so a client can tell an
    /// abandoned run from a broken one, and so the audit trail records why the
    /// run stopped.
    Cancelled,
    /// Finished with an error; cannot be resumed.
    Failed,
}

impl RunStatus {
    /// Returns whether a run in this state may be resumed.
    pub fn resumable(&self) -> bool {
        matches!(self, RunStatus::Running | RunStatus::Suspended)
    }

    /// Returns whether the run reached a state it will not leave on its own.
    pub fn is_terminal(&self) -> bool {
        !matches!(self, RunStatus::Running)
    }
}

/// A serializable snapshot of a paused execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionCheckpoint {
    /// Scheduling boundary format. Missing on legacy records whose successors
    /// may not have been queued yet; those records remain readable, but cannot
    /// safely be resumed automatically.
    #[serde(default)]
    pub transition_version: u32,
    /// Legacy records have no evidence about their admitted package set. They
    /// remain readable, but cannot prove that resuming will keep the same code.
    #[serde(default)]
    pub addon_identity_version: u32,
    #[serde(default)]
    pub addon_packages: std::collections::BTreeMap<String, crate::addon::package::Identity>,
    #[serde(default)]
    pub function_identities: Option<std::collections::BTreeMap<String,String>>,
    #[serde(default)]
    pub view: super::view::ExecutionView,
    /// An executor may have started, but its outcome is not committed. Never
    /// replay it automatically: external effects may already have happened.
    #[serde(default)]
    pub in_flight: Option<NodeId>,
    /// The run identifier.
    pub run_id: uuid::Uuid,
    /// The blueprint being executed.
    pub blueprint_id: uuid::Uuid,
    #[serde(default)]
    pub blueprint_version: Option<crate::storage::versioning::VersionRef>,
    /// The run status.
    pub status: RunStatus,
    /// Execution start time in milliseconds since the Unix epoch.
    pub started_at: u64,
    /// The time this checkpoint was written.
    pub updated_at: u64,
    /// The execution call stack.
    pub call_stack: Vec<Frame>,
    /// Values produced on data output pins.
    pub data_values: HashMap<PinId, Value>,
    /// Nodes that completed execution.
    pub executed: Vec<NodeId>,
    /// Nodes waiting to run (the work queue).
    pub pending: Vec<NodeId>,
    /// Nodes ever triggered during the run.
    pub triggered: Vec<NodeId>,
    /// Mutations recorded so far, for rollback.
    pub transaction_log: Vec<TransactionEntry>,
    /// Completion order of the root scheduler (retry segment boundary).
    #[serde(default)]
    pub executed_order: Vec<NodeId>,
    /// Failed validation attempts per validator node.
    #[serde(default)]
    pub attempt_counts: HashMap<NodeId, u32>,
    /// Rollback boundaries per validator node, refreshed on each pass.
    #[serde(default)]
    pub validation_marks: HashMap<NodeId, super::context::RetryMark>,
    /// Frame-scoped variable maps, innermost last (mirrors the call stack).
    #[serde(default)]
    pub variables: Vec<HashMap<String, Value>>,
    /// Active ForEach loops, innermost last.
    #[serde(default)]
    pub foreach_stack: Vec<super::context::ForEachState>,
    /// The agent's task list at this point in the run.
    #[serde(default)]
    pub todos: Vec<metteur_shared::llm::TodoItem>,
    /// The agent execution tree of the run.
    #[serde(default)]
    pub exec_tree: super::tree::ExecTree,
    /// Tree ids of entered function frames, innermost last.
    #[serde(default)]
    pub frame_trees: Vec<String>,
    /// Tree id of the node in flight when the checkpoint was taken.
    ///
    /// Restoring the parent frame is what keeps post-resume nodes (and any
    /// sub-agent a resumed node spawns) attached to the right branch instead of
    /// re-parenting them under a stale frame root.
    #[serde(default)]
    pub current_tree: Option<String>,
    /// Consecutive validation failures seen so far, feeding the circuit breaker.
    ///
    /// Persisted so a resumed run cannot reset the counter and slip past the
    /// replan threshold it had already reached.
    #[serde(default)]
    pub circuit_failures: u32,
    /// The failure reason, if the run failed.
    pub error: Option<String>,
}

impl ExecutionCheckpoint {
    pub(crate) fn ensure_addons(&self, registry: &crate::registry::Registry) -> DaemonResult<()> {
        if self.addon_identity_version != 1 {
            return Err(DaemonError::Addon("Checkpoint has no verifiable addon identity record; start a new run explicitly".into()));
        }
        if self.addon_packages != registry.addon_packages {
            return Err(DaemonError::Addon("Addon identity, version, scope or content differs from checkpoint; start a new run explicitly".into()));
        }
        let functions=registry.function_identities();
        if self.function_identities.as_ref().is_some_and(|saved|saved!=&functions) || (self.function_identities.is_none() && !functions.is_empty()) {
            return Err(DaemonError::Addon("Function identity or body differs from checkpoint, or legacy evidence is missing; start a new run explicitly".into()));
        }
        Ok(())
    }

    /// Builds an initial resumable checkpoint for a fresh run.
    pub fn running(run_id: uuid::Uuid, blueprint_id: uuid::Uuid, started_at: u64) -> Self {
        Self {
            transition_version: CHECKPOINT_TRANSITION_VERSION,
            addon_identity_version: 1,
            addon_packages: Default::default(),
            function_identities: None,
            view: Default::default(),
            in_flight: None,
            run_id,
            blueprint_id,
            blueprint_version: None,
            status: RunStatus::Running,
            started_at,
            updated_at: started_at,
            call_stack: Vec::new(),
            data_values: HashMap::new(),
            executed: Vec::new(),
            pending: Vec::new(),
            triggered: Vec::new(),
            transaction_log: Vec::new(),
            executed_order: Vec::new(),
            attempt_counts: HashMap::new(),
            validation_marks: HashMap::new(),
            variables: vec![HashMap::new()],
            foreach_stack: Vec::new(),
            todos: Vec::new(),
            exec_tree: super::tree::ExecTree::new(),
            frame_trees: Vec::new(),
            current_tree: None,
            circuit_failures: 0,
            error: None,
        }
    }
}

/// A target for persisting execution checkpoints.
pub trait CheckpointSink: Send + Sync {
    /// The run id this sink writes for.
    fn run_id(&self) -> uuid::Uuid;

    /// Persists a checkpoint, overwriting the previous one for the run.
    fn write(&self, checkpoint: &ExecutionCheckpoint) -> DaemonResult<()>;
}

/// A checkpoint sink backed by the workspace RocksDB.
#[derive(Clone)]
pub struct DbCheckpointSink {
    db: Db,
    run_id: uuid::Uuid,
}

impl DbCheckpointSink {
    /// Creates a sink writing for `run_id`.
    pub fn new(db: Db, run_id: uuid::Uuid) -> Self {
        Self {
            db,
            run_id,
        }
    }

    /// Loads the checkpoint for `run_id`, if any.
    pub fn load(db: &Db, run_id: uuid::Uuid) -> DaemonResult<Option<ExecutionCheckpoint>> {
        let data = match db.get(cf::EXECUTION_STATE, run_id.as_bytes())? {
            Some(data) => data,
            None => return Ok(None),
        };
        let checkpoint: ExecutionCheckpoint =
            serde_json::from_slice(&data).map_err(|e| DaemonError::Serialization(e.to_string()))?;
        Ok(Some(checkpoint))
    }

    /// Lists all checkpoints in the database, oldest first.
    pub fn list(db: &Db) -> DaemonResult<Vec<ExecutionCheckpoint>> {
        let mut out = Vec::new();
        for (key, value) in db.scan(cf::EXECUTION_STATE)? {
            if key.starts_with(crate::replan::application::INTENT_PREFIX)
                || key.starts_with(b"oversight:")
            {
                continue;
            }
            let checkpoint: ExecutionCheckpoint = serde_json::from_slice(&value)
                .map_err(|e| DaemonError::Serialization(e.to_string()))?;
            out.push(checkpoint);
        }
        out.sort_by_key(|c| c.started_at);
        Ok(out)
    }
}

impl CheckpointSink for DbCheckpointSink {
    fn run_id(&self) -> uuid::Uuid {
        self.run_id
    }

    fn write(&self, checkpoint: &ExecutionCheckpoint) -> DaemonResult<()> {
        let _guard = self
            .db
            .oversight_gate
            .lock()
            .map_err(|_| DaemonError::Persistence("oversight lock poisoned".into()))?;
        if !checkpoint.status.resumable() {
            crate::oversight::requests::close_locked(&self.db, self.run_id)?;
        }
        let data = serde_json::to_vec(checkpoint)
            .map_err(|e| DaemonError::Serialization(e.to_string()))?;
        self.db.put_durable(cf::EXECUTION_STATE, self.run_id.as_bytes(), &data)?;
        if let Err(error) = crate::oversight::scheduler::checkpoint_locked(&self.db, checkpoint) {
            tracing::warn!(%error, "review trigger could not be recorded");
        }
        Ok(())
    }
}
