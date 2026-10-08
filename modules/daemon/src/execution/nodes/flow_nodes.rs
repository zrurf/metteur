//! Flow-support node executors that need no interpreter scheduling changes.

use std::collections::HashMap;

use async_trait::async_trait;
use metteur_shared::{Node, PinId, Value};

use crate::error::DaemonResult;
use crate::execution::context::ExecutionContext;
use crate::registry::NodeExecutor;

use super::int_input;

/// Pauses execution for a number of milliseconds.
pub struct DelayExecutor;

#[async_trait]
impl NodeExecutor for DelayExecutor {
    fn kind(&self) -> &str {
        "Delay"
    }

    async fn execute(
        &self,
        node: &Node,
        inputs: &HashMap<PinId, Value>,
        _ctx: &mut ExecutionContext,
    ) -> DaemonResult<HashMap<PinId, Value>> {
        let ms = int_input(node, inputs, "Ms")?.max(0) as u64;
        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
        Ok(HashMap::new())
    }
}

/// Pauses execution until a user approves or denies the request.
///
/// The decision is published as an `approval_request` execution event and
/// resolved through the shared broker. The outcome is exposed on the `Allowed`
/// data pin, and the interpreter routes the `Approved` or `Denied` execution
/// branch from it.
pub struct RequestApprovalExecutor;

#[async_trait]
impl NodeExecutor for RequestApprovalExecutor {
    fn kind(&self) -> &str {
        "RequestApproval"
    }

    async fn execute(
        &self,
        node: &Node,
        inputs: &HashMap<PinId, Value>,
        ctx: &mut ExecutionContext,
    ) -> DaemonResult<HashMap<PinId, Value>> {
        let message = super::string_input(node, inputs, "Message")?;
        let allow = crate::replan::await_approval(
            ctx,
            "node_approval",
            &message,
            serde_json::json!({ "node_id": node.id.to_string() }),
        )
        .await?;
        super::bool_output(node, "Allowed", allow)
    }
}

/// The interpreter owns this gate so approved edits commit before it releases.
pub struct OversightCheckpointExecutor;
#[async_trait]
impl NodeExecutor for OversightCheckpointExecutor {
    fn kind(&self) -> &str {
        "OversightCheckpoint"
    }
    async fn execute(
        &self,
        _node: &Node,
        _inputs: &HashMap<PinId, Value>,
        _ctx: &mut ExecutionContext,
    ) -> DaemonResult<HashMap<PinId, Value>> {
        Err(crate::DaemonError::Execution(
            "OversightCheckpoint requires an interpreter boundary".into(),
        ))
    }
}
