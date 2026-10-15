//! Prompt resource output comes only from the immutable admitted run manifest.
use std::collections::HashMap;
use metteur_shared::{Node, PinId, Value};
use crate::{DaemonError, DaemonResult, execution::context::ExecutionContext, registry::NodeExecutor};
pub struct PromptResourceExecutor;
#[async_trait::async_trait]
impl NodeExecutor for PromptResourceExecutor {
    fn kind(&self)->&str { "PromptResource" }
    async fn execute(&self,node:&Node,_inputs:&HashMap<PinId,Value>,ctx:&mut ExecutionContext)->DaemonResult<HashMap<PinId,Value>> {
        let db=ctx.workspace_db.clone().ok_or_else(|| DaemonError::Execution("Prompt resource requires workspace storage".into()))?;
        let snapshot=crate::prompt_snapshots::SelectionStore::new(db,&ctx.workspace_root).and_then(|store| store.run_snapshot(ctx.run_id,node.id)).map_err(|error| DaemonError::Execution(error.to_string()))?;
        super::string_output(node,"Text",snapshot.body)
    }
}
