//! Bounded host-bound execution facts, not a workspace-wide private session search.
use crate::{
    DaemonError, DaemonResult,
    execution::{DbCheckpointSink, context::ExecutionContext},
    registry::Tool,
};
use async_trait::async_trait;
use metteur_shared::Value;
pub struct ReadRunEvidence;
#[async_trait]
impl Tool for ReadRunEvidence {
    fn name(&self) -> &str {
        "ReadRunEvidence"
    }
    fn description(&self) -> &str {
        "Read the current host-bound blueprint's recorded status, version and validation checks. No caller-selected run/session, no execution or approval, no private conversation text. Terminal success is not user acceptance."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","additionalProperties":false,"properties":{}})
    }
    fn max_result_bytes(&self) -> usize {
        16 * 1024
    }
    // Leave serialized: the read-only parallel child has a different host binding.
    async fn call(&self, args: &[Value], ctx: &mut ExecutionContext) -> DaemonResult<Value> {
        if !crate::registry::capabilities::allowed(ctx, self.name()) {
            return Err(DaemonError::PermissionDenied(
                "Run evidence unavailable to this host role".into(),
            ));
        }
        let [Value::Json(input)] = args else {
            return Err(DaemonError::Execution("Expected an empty object".into()));
        };
        if input.as_object().is_none_or(|v| !v.is_empty()) {
            return Err(DaemonError::PermissionDenied(
                "Caller-selected run IDs are not allowed".into(),
            ));
        }
        let db = ctx
            .workspace_db
            .as_ref()
            .ok_or_else(|| DaemonError::Execution("Open workspace required".into()))?;
        let (run, launch) = if ctx.blueprint.is_some() {
            (Some(ctx.run_id), None)
        } else if let Some(handle) = &ctx.continuation {
            let record = handle
                .store
                .get(handle.id)?
                .ok_or_else(|| DaemonError::NotFound("Parent task unavailable".into()))?;
            if record.owner_subject != ctx.user {
                return Err(DaemonError::PermissionDenied("Task belongs to another owner".into()));
            }
            if let Some(id) = record.launch_intent {
                let (_, intent) =
                    crate::continuation::launch::LaunchStore::new(db.clone(), &ctx.workspace_root)?
                        .get(id, &ctx.user)?;
                (
                    intent.run,
                    Some(
                        serde_json::json!({"intent_id":id,"state":intent.state,"attention":intent.attention,"resume_on_completion":intent.resume_on_completion}),
                    ),
                )
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };
        let checkpoint = run.map(|id| DbCheckpointSink::load(db, id)).transpose()?.flatten();
        let output = if let Some(cp) = checkpoint {
            let checks:Vec<_>=cp.view.invocations.iter().filter(|v|v.check.is_some()).take(64).map(|v|serde_json::json!({"node_id":v.node_id,"status":v.status,"check":v.check,"current":v.current,"version":v.version})).collect();
            use crate::execution::transaction::TransactionEntry;
            let mut changes=Vec::new();let mut change_count=0usize;
            for entry in &cp.transaction_log {
                let item=match entry {
                    TransactionEntry::FileOperation{operation_id,..}|TransactionEntry::BlueprintApplication{operation_id}=>{
                        change_count+=1;if changes.len()>=64{continue}
                        let operation=db.get(crate::storage::persistence::cf::FILE_INTENTS,operation_id.as_bytes())?.and_then(|raw|serde_json::from_slice::<crate::execution::file_journal::FileOperation>(&raw).ok()).filter(|op|op.origin.run_id==cp.run_id);
                        Some(match operation {
                            Some(op)=>serde_json::json!({"operation_id":operation_id,"path":op.path.strip_prefix(&ctx.workspace_root).ok().map(|p|p.to_string_lossy().into_owned()),"phase":op.phase,"before":op.before,"after":op.after,"diff_reference":"workspace_file_journal"}),
                            None=>serde_json::json!({"operation_id":operation_id,"status":"unavailable"})
                        })
                    },
                    TransactionEntry::FileWrite{path,old_content,new_content,reverted}=>{
                        change_count+=1;Some(serde_json::json!({"path":path.strip_prefix(&ctx.workspace_root).ok().map(|p|p.to_string_lossy().into_owned()),"operation":"write","before_bytes":old_content.as_ref().map(Vec::len),"after_bytes":new_content.len(),"reverted":reverted}))
                    },
                    TransactionEntry::FileDelete{path,content,reverted}=>{
                        change_count+=1;Some(serde_json::json!({"path":path.strip_prefix(&ctx.workspace_root).ok().map(|p|p.to_string_lossy().into_owned()),"operation":"delete","before_bytes":content.len(),"reverted":reverted}))
                    },
                    TransactionEntry::ToolCall{..}=>None
                };
                if changes.len()<64 {if let Some(item)=item{changes.push(item);}}
            }
            serde_json::json!({"run_id":cp.run_id,"status":cp.status,"version":cp.blueprint_version,"in_flight":cp.in_flight,"executed_nodes":cp.executed.len(),"recorded_checks":checks,"recorded_file_changes":changes,"file_change_count":change_count,"changes_truncated":change_count>64,"change_coverage":"recorded_file_journal_only; shell and network effects may be untracked","launch":launch,"origin":"host_checkpoint","user_acceptance":"not_asserted"})
        } else {
            serde_json::json!({"run_id":run,"launch":launch,"status":"unavailable","origin":"host_checkpoint","user_acceptance":"not_asserted"})
        };
        let text =
            crate::observability::anon::Anonymizer::new(&[]).anonymize(&output.to_string()).await;
        Ok(Value::Json(
            serde_json::from_str(&text)
                .unwrap_or_else(|_| serde_json::json!({"status":"redacted"})),
        ))
    }
}
