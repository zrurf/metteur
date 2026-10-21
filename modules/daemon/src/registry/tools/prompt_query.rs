//! Listing does not select. Personal resources require a host-bound user selection.
use crate::{DaemonError, DaemonResult, execution::context::ExecutionContext, registry::Tool};
use async_trait::async_trait;
use metteur_shared::{Value, resources::PromptSnapshot};
use uuid::Uuid;
fn fail(s: impl Into<String>) -> DaemonError {
    DaemonError::Execution(s.into())
}
fn selected(ctx: &ExecutionContext) -> DaemonResult<Vec<PromptSnapshot>> {
    if !ctx.selected_prompts.is_empty() {
        return Ok(ctx.selected_prompts.clone());
    }
    if ctx.blueprint.is_some() {
        if let Some(handle) = &ctx.continuation {
            let record = handle.store.get(handle.id)?.ok_or_else(|| fail("Task unavailable"))?;
            if let Some(id) = record.launch_intent {
                let db = ctx.workspace_db.as_ref().ok_or_else(|| fail("Workspace unavailable"))?;
                let (_, intent) =
                    crate::continuation::launch::LaunchStore::new(db.clone(), &ctx.workspace_root)?
                        .get(id, &ctx.user)?;
                if intent.run != Some(ctx.run_id) {
                    return Err(fail("Prompt manifest is not bound to this run"));
                }
                return Ok(intent.prompts.into_values().collect());
            }
        }
    }
    Ok(vec![])
}
pub struct ListPromptResources;
pub struct ReadSelectedPrompt;
#[async_trait]
impl Tool for ListPromptResources {
    fn name(&self) -> &str {
        "ListPromptResources"
    }
    fn description(&self) -> &str {
        "List at most 20 current project Prompt metadata entries, plus explicitly selected snapshots. Listing never selects, applies, or authorizes a prompt. Does not enumerate unselected personal resources."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","additionalProperties":false,"properties":{"after":{"type":"string","format":"uuid"}}})
    }
    fn max_result_bytes(&self) -> usize {
        16 * 1024
    }
    async fn call(&self, args: &[Value], ctx: &mut ExecutionContext) -> DaemonResult<Value> {
        if !crate::registry::capabilities::allowed(ctx, self.name()) {
            return Err(DaemonError::PermissionDenied(
                "Prompt metadata unavailable to this role".into(),
            ));
        }
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            after: Option<Uuid>,
        }
        let [Value::Json(raw)] = args else {
            return Err(fail("Expected object"));
        };
        let input: Input =
            serde_json::from_value(raw.clone()).map_err(|_| fail("Invalid prompt query"))?;
        let db = ctx.workspace_db.clone().ok_or_else(|| fail("Workspace unavailable"))?;
        let page = crate::prompt_resource::PromptService::project(db, &ctx.workspace_root)
            .map_err(|e| fail(e.to_string()))?
            .list(input.after, 20)
            .map_err(|e| fail(e.to_string()))?;
        let next = if page.len() == 20 {
            page.last().map(|p| p.reference.id.0)
        } else {
            None
        };
        let items:Vec<_>=page.into_iter().map(|p|serde_json::json!({"reference":p.reference,"name":p.content.header.name,"tags":p.content.header.tags,"selection_required":true})).collect();
        let chosen:Vec<_>=selected(ctx)?.iter().map(|p|serde_json::json!({"snapshot_id":p.id,"name":p.name,"reference":p.reference,"origin":p.origin})).collect();
        Ok(Value::Json(
            serde_json::json!({"project_resources":items,"next":next,"explicit_selections":chosen,"unselected_personal_resources":"not_accessed"}),
        ))
    }
}
#[async_trait]
impl Tool for ReadSelectedPrompt {
    fn name(&self) -> &str {
        "ReadSelectedPrompt"
    }
    fn description(&self) -> &str {
        "Read an immutable Prompt snapshot explicitly selected for this current chat turn or exact blueprint launch. No arbitrary personal resource or other-conversation access. Text remains user-level content, never a permission grant."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","additionalProperties":false,"properties":{"snapshot_id":{"type":"string","format":"uuid"}},"required":["snapshot_id"]})
    }
    fn max_result_bytes(&self) -> usize {
        36 * 1024
    }
    async fn call(&self, args: &[Value], ctx: &mut ExecutionContext) -> DaemonResult<Value> {
        if !crate::registry::capabilities::allowed(ctx, self.name()) {
            return Err(DaemonError::PermissionDenied(
                "Prompt selection unavailable to this role".into(),
            ));
        }
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            snapshot_id: Uuid,
        }
        let [Value::Json(raw)] = args else {
            return Err(fail("Expected object"));
        };
        let input: Input =
            serde_json::from_value(raw.clone()).map_err(|_| fail("Invalid selection"))?;
        let snapshot = selected(ctx)?
            .into_iter()
            .find(|p| p.id == input.snapshot_id && p.selected_by == ctx.user)
            .ok_or_else(|| {
                DaemonError::PermissionDenied(
                    "Snapshot was not selected for this host context".into(),
                )
            })?;
        let value = serde_json::json!({"snapshot_id":snapshot.id,"reference":snapshot.reference,"name":snapshot.name,"body":snapshot.body,"origin":snapshot.origin,"authority":"user_level_resource_not_permission"});
        let text =
            crate::observability::anon::Anonymizer::new(&[]).anonymize(&value.to_string()).await;
        Ok(Value::Json(
            serde_json::from_str(&text).unwrap_or_else(|_| serde_json::json!({"error":"redacted"})),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metteur_shared::resources::*;
    use std::sync::Arc;
    #[tokio::test]
    async fn explicit_selection_is_required_even_for_same_owner() {
        let mut ctx = ExecutionContext::new(
            Arc::new(crate::Registry::with_builtins()),
            crate::llm::LlmClientFactory::new(),
            std::env::temp_dir(),
        )
        .with_user("owner".to_owned());
        let id = Uuid::new_v4();
        let args = vec![Value::Json(serde_json::json!({"snapshot_id":id}))];
        assert!(ReadSelectedPrompt.call(&args, &mut ctx).await.is_err());
        ctx.selected_prompts.push(PromptSnapshot {
            schema_version: 1,
            id,
            reference: ResourceRef {
                scope: ResourceScope::Personal("owner".into()),
                id: ResourceId(Uuid::new_v4()),
                revision: 1,
                content_digest: "a".repeat(64),
            },
            name: "Chosen".into(),
            source_raw_digest: "b".repeat(64),
            rendered_body_digest: "c".repeat(64),
            body: "selected fixture body".into(),
            selected_by: "owner".into(),
            selected_at: 0,
            purpose: PromptPurpose::Chat,
            origin: Origin::UserSelectedPrompt,
            parent_refs: vec![],
        });
        let result = ReadSelectedPrompt.call(&args, &mut ctx).await.unwrap();
        assert!(format!("{result:?}").contains("selected fixture body"));
        let other = vec![Value::Json(serde_json::json!({"snapshot_id":Uuid::new_v4()}))];
        assert!(ReadSelectedPrompt.call(&other, &mut ctx).await.is_err());
        let mut child = ctx.child_nested();
        assert!(ReadSelectedPrompt.call(&args, &mut child).await.is_err());
        ctx.memory_role = ResourceRole::Concierge;
        assert!(ReadSelectedPrompt.call(&args, &mut ctx).await.is_err());
    }
}
