//! Close ACK must follow cleanup of real Job source receipts holding RocksDB.
use metteur_daemon::{
    AppState, DaemonService, Registry, WorkspaceManager,
    continuation::{
        policy::Policy,
        state::Terminal,
        store::{ContinuationStore, TaskHandle},
    },
    execution::context::ExecutionContext,
    llm::LlmClientFactory,
    sandbox::PermissionMode,
};
use metteur_proto::proto::{CloseWorkspaceRequest, daemon_server::Daemon};
use metteur_shared::{Value, config::Config};
use std::sync::Arc;
use tonic::Request;
use uuid::Uuid;

#[tokio::test]
async fn closing_a_running_job_finishes_its_source_receipt_before_immediate_reopen() {
    let root = std::env::temp_dir().join(format!("dw-close-job-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let registry = Arc::new(Registry::with_builtins());
    let state = Arc::new(AppState::new(
        WorkspaceManager::new().with_global_config_path(root.join("global.toml")),
        registry.clone(),
        Config::default(),
    ));
    let ws = state.workspaces.open(&root).await.unwrap();
    ws.config.write().await.sandbox.enabled = false;
    let store = ContinuationStore::new(ws.db.clone(), ws.root()).unwrap();
    let policy = Policy {
        detached: true,
        max_turns: 4,
        token_limit: 300000,
        expires_ms: chrono::Utc::now().timestamp_millis() as u64 + 60000,
        purpose: "Close synthetic detached Job".into(),
    };
    let record = store.create(policy.task(Uuid::new_v4(), 0)).unwrap();
    let id = record.task.id;
    let mut ctx = ExecutionContext::new(registry.clone(), LlmClientFactory::new(), root.clone());
    ctx.jobs = ws.jobs();
    ctx.workspace_db = Some(ws.db.clone());
    ctx.config = Some(ws.config.clone());
    ctx.permission_mode = PermissionMode::Sandbox;
    ctx.continuation = Some(TaskHandle {
        store: store.clone(),
        id,
        generation: 1,
    });
    let command = if cfg!(windows) {
        "ping -n 30 127.0.0.1"
    } else {
        "sleep 30"
    };
    registry
        .tool("StartCommand")
        .unwrap()
        .call(&[Value::Json(serde_json::json!({"command":command}))], &mut ctx)
        .await
        .unwrap();
    assert_eq!(store.get(id).unwrap().unwrap().task.sources.len(), 1);
    drop(ctx);
    drop(store);
    drop(ws);
    let service = DaemonService::new(state.clone());
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        service.close_workspace(Request::new(CloseWorkspaceRequest {
            path: root.to_string_lossy().into(),
        })),
    )
    .await
    .unwrap()
    .unwrap();
    let reopened = state.workspaces.open(&root).await.unwrap();
    let store = ContinuationStore::new(reopened.db.clone(), reopened.root()).unwrap();
    let recovered = store.get(id).unwrap().unwrap();
    assert_eq!(recovered.task.budget.used_turns, record.task.budget.used_turns);
    assert_eq!(recovered.task.sources.values().next().unwrap().terminal, Some(Terminal::Cancelled));
}

#[tokio::test]
async fn closing_a_readonly_child_drains_its_receipt_and_keeps_unknown_usage_before_reopen() {
    let root = std::env::temp_dir().join(format!("dw-close-child-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let registry = Arc::new(Registry::with_builtins());
    let state = Arc::new(AppState::new(
        WorkspaceManager::new().with_global_config_path(root.join("global.toml")),
        registry.clone(),
        Config::default(),
    ));
    let ws = state.workspaces.open(&root).await.unwrap();
    let store = ContinuationStore::new(ws.db.clone(), ws.root()).unwrap();
    let policy = Policy {
        detached: true,
        max_turns: 4,
        token_limit: 2_000_000,
        expires_ms: chrono::Utc::now().timestamp_millis() as u64 + 60000,
        purpose: "Close synthetic readonly child".into(),
    };
    let row = store.create(policy.task(Uuid::new_v4(), 0)).unwrap();
    let id = row.task.id;
    store
        .update(id, row.revision, |row| {
            row.policy = Some(policy);
            row.model_options = serde_json::json!({"provider":"openai-chat","model":"fixture"});
            Ok(())
        })
        .unwrap();
    let model = metteur_daemon::llm::MockClient::new_delayed(
        vec![metteur_daemon::llm::MockStep::Text("late opinion".into())],
        std::time::Duration::from_secs(30),
    );
    let mut ctx = ExecutionContext::new(
        registry.clone(),
        LlmClientFactory::with_override(Arc::new(model)),
        root.clone(),
    );
    ctx.jobs = ws.jobs();
    ctx.workspace_db = Some(ws.db.clone());
    ctx.continuation = Some(TaskHandle {
        store: store.clone(),
        id,
        generation: 1,
    });
    registry
        .tool("StartSubAgent")
        .unwrap()
        .call(
            &[Value::Json(
                serde_json::json!({"task":"Wait for workspace cancellation without tool effects"}),
            )],
            &mut ctx,
        )
        .await
        .unwrap();
    let entered = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while store.get(id).unwrap().unwrap().task.budget.reserved_tokens == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(
        entered.is_ok(),
        "Child did not enter its call: {:?}",
        store.get(id).unwrap().unwrap().task
    );
    drop(ctx);
    drop(store);
    drop(ws);
    let service = DaemonService::new(state.clone());
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        service.close_workspace(Request::new(CloseWorkspaceRequest {
            path: root.to_string_lossy().into(),
        })),
    )
    .await
    .unwrap()
    .unwrap();
    let reopened = state.workspaces.open(&root).await.unwrap();
    let recovered = ContinuationStore::new(reopened.db.clone(), reopened.root())
        .unwrap()
        .get(id)
        .unwrap()
        .unwrap();
    assert_eq!(recovered.task.sources.values().next().unwrap().terminal, Some(Terminal::Cancelled));
    assert!(recovered.task.budget.unknown_usage && recovered.task.budget.reserved_tokens > 0);
    assert_eq!(recovered.task.budget.used_turns, 1);
}
