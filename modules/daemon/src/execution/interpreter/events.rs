//! Events emitted during blueprint execution.

use metteur_shared::llm::{ContextRegion, TodoItem};
use metteur_shared::{NodeId, PinId, Value};

/// An event emitted during blueprint execution.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum ExecutionEvent {
    /// Trusted review lifecycle notification; model text cannot emit this.
    Oversight { review_id: String, detail: String },
    /// A node started executing.
    NodeStarted {
        node_id: NodeId,
    },
    /// A node finished executing.
    NodeFinished {
        node_id: NodeId,
    },
    /// A node finished and reported its produced data output values.
    NodeData {
        node_id: NodeId,
        outputs: Vec<(PinId, Value)>,
        /// The function whose body this node belongs to, if any.
        function: Option<uuid::Uuid>,
    },
    /// A node produced a message.
    Message {
        node_id: NodeId,
        message: String,
    },
    /// The sandbox requests user approval for an operation.
    ApprovalRequested {
        node_id: NodeId,
        request_id: String,
        detail: String,
    },
    /// A CallLLM node finished and reported its context composition.
    ContextUsage {
        node_id: NodeId,
        regions: Vec<ContextRegion>,
    },
    /// The agent's task list changed.
    Todos {
        node_id: NodeId,
        todos: Vec<TodoItem>,
    },
    /// A background command changed state.
    ///
    /// Emitted when a job is started and when its completion is observed (by a
    /// wait, or by the engine waking a parked turn); a job that finishes while
    /// nobody is looking is reported the next time it is observed.
    Job {
        node_id: NodeId,
        job_id: String,
        /// `started` or `finished`.
        state: String,
        /// One-line summary: job id, state, duration, size, command.
        summary: String,
    },
}
