//! Host role limits. Model names and tool schemas cannot grant capabilities.
use crate::execution::context::ExecutionContext;
use metteur_shared::resources::ResourceRole;
pub const READ_ONLY: &[&str] = &[
    "ReadFile",
    "ListDirectory",
    "SearchFile",
    "Grep",
    "Glob",
    "GetDependencies",
    "SearchProjectMemory",
    "ReadProjectMemory",
    "ReadRunEvidence",
];
pub fn role(ctx: &ExecutionContext) -> &'static str {
    if ctx.read_only_tools {
        "read_only_child"
    } else {
        match ctx.memory_role {
            ResourceRole::Supervisor => "supervisor",
            ResourceRole::Concierge => "concierge",
            _ => {
                if ctx.depth > 0 {
                    "synchronous_child"
                } else if ctx.blueprint.is_some() {
                    "blueprint_executor"
                } else {
                    "main_executor"
                }
            }
        }
    }
}
pub fn allowed(ctx: &ExecutionContext, name: &str) -> bool {
    if ctx.read_only_tools && !READ_ONLY.contains(&name) {
        return false;
    }
    if matches!(ctx.memory_role, ResourceRole::Concierge | ResourceRole::Supervisor) {
        return false;
    } // These roles have dedicated non-general dispatchers.
    match name {
        "RequestBlueprintRun" => {
            ctx.chat_session.is_some() && ctx.depth == 0 && ctx.blueprint.is_none()
        }
        "WriteProjectNote" => {
            ctx.depth == 0
                && matches!(ctx.memory_role, ResourceRole::MainAgent | ResourceRole::User)
        }
        "ListPromptResources" => ctx.depth == 0 && !ctx.read_only_tools,
        "ReadSelectedPrompt" => {
            !ctx.selected_prompts.is_empty()
                || (ctx.blueprint.is_some() && ctx.continuation.is_some())
        }
        "ReadRunEvidence" => ctx.blueprint.is_some() || ctx.continuation.is_some(),
        "ReadHandoff" => ctx.handoff_ref.is_some(),
        "AwaitEvents" | "StartSubAgent" | "ReadTaskSource" => ctx.allow_deferred_wait,
        _ => true,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    #[test]
    fn read_only_role_cannot_gain_tools_from_an_allow_list_or_model() {
        let mut ctx = ExecutionContext::new(
            Arc::new(crate::Registry::with_builtins()),
            crate::llm::LlmClientFactory::new(),
            std::env::temp_dir(),
        );
        ctx.read_only_tools = true;
        ctx.allow_deferred_wait = true;
        ctx.chat_session = Some(uuid::Uuid::new_v4());
        for name in [
            "WriteFile",
            "ExecuteCommand",
            "StartSubAgent",
            "RequestBlueprintRun",
            "WriteProjectNote",
            "AwaitEvents",
        ] {
            assert!(!allowed(&ctx, name), "{name}");
        }
        assert!(allowed(&ctx, "ReadFile"));
        assert!(ctx.child_nested().read_only_tools);
        ctx.read_only_tools = false;
        ctx.memory_role = ResourceRole::Concierge;
        assert!(!allowed(&ctx, "ReadFile"));
        ctx.memory_role = ResourceRole::Supervisor;
        assert!(!allowed(&ctx, "ExecuteCommand"));
    }
    #[tokio::test]
    async fn readonly_limit_is_enforced_by_blueprint_tool_dispatch_too() {
        use crate::registry::NodeExecutor;
        let root = std::env::temp_dir().join(format!("nx-m4-role-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let registry = Arc::new(crate::Registry::with_builtins());
        let graph=metteur_shared::dsl::compile_draft_value_with_catalog(&serde_json::json!({"name":"Role boundary","nodes":{"start":{"kind":"Start"},"write":{"kind":"WriteFile","path":"blocked.txt","content":"blocked"}},"flow":["start -> write"]}),&registry.authoring_catalog()).unwrap();
        let node = graph.nodes.iter().find(|n| n.kind == "Tool").unwrap();
        let mut ctx =
            ExecutionContext::new(registry, crate::llm::LlmClientFactory::new(), root.clone());
        ctx.read_only_tools = true;
        assert!(
            crate::execution::nodes::tool::ToolExecutor
                .execute(node, &std::collections::HashMap::new(), &mut ctx)
                .await
                .is_err()
        );
        assert!(!root.join("blocked.txt").exists());
    }
}
