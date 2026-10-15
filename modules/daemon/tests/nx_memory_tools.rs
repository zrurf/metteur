use metteur_daemon::{Registry, execution::context::ExecutionContext, llm::LlmClientFactory, storage::persistence::Db, registry::{Tool, tools::memory::{WriteProjectNote, ReadProjectMemory}}};
use metteur_shared::{Value, resources::ResourceRole};
use uuid::Uuid;
#[tokio::test]
async fn registered_tools_enforce_host_role_and_strict_arguments() {
    let root = std::env::temp_dir().join(format!("nx-memory-tools-{}", Uuid::new_v4()));
    std::fs::create_dir_all(root.join(".metteur")).unwrap();
    let mut ctx = ExecutionContext::new(std::sync::Arc::new(Registry::with_builtins()), LlmClientFactory::new(), root.clone());
    ctx.workspace_db = Some(Db::open(&root.join(".metteur/db")).unwrap());
    let id = Uuid::new_v4();
    let input = serde_json::json!({"request_id":Uuid::new_v4(),"expected_revision":0,"note":{"id":id,"title":"Test","body":"I believe this works"}});
    for role in [ResourceRole::Supervisor, ResourceRole::Concierge, ResourceRole::ReadOnlyAgent] {
        ctx.memory_role = role;
        assert!(WriteProjectNote.call(&[Value::Json(input.clone())], &mut ctx).await.is_err());
    }
    ctx.memory_role = ResourceRole::MainAgent;
    ctx.depth = 1;
    assert!(WriteProjectNote.call(&[Value::Json(input.clone())], &mut ctx).await.is_err());
    ctx.depth = 0;
    let value = WriteProjectNote.call(&[Value::Json(input.clone())], &mut ctx).await.unwrap();
    assert!(format!("{value:?}").contains("model_opinion"));
    let mut forged = input; forged["role"] = "user".into();
    assert!(WriteProjectNote.call(&[Value::Json(forged)], &mut ctx).await.is_err());
    assert!(ReadProjectMemory.call(&[Value::Json(serde_json::json!({"id":id}))], &mut ctx).await.is_ok());
}
