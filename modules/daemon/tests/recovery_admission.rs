//! Recovery failures must be visible before a chat can start file effects.
use metteur_daemon::execution::file_journal::{DbFileJournal, FileJournal, FileOrigin};
use metteur_daemon::storage::persistence::cf;
use metteur_daemon::{AppState, DaemonService, Registry, WorkspaceManager};
use metteur_proto::proto::{SendChatRequest, daemon_server::Daemon};
use metteur_shared::config::Config;
use std::sync::Arc;
use tokio_stream::StreamExt;
use tonic::Request;
use uuid::Uuid;

#[tokio::test]
async fn recovery_conflict_blocks_chat_until_resolved_without_leaking_a_run_slot() {
    let root = std::env::temp_dir().join(format!("metteur-admission-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let state = Arc::new(AppState::new(
        WorkspaceManager::new().with_global_config_path(root.join(".metteur/global.toml")),
        Arc::new(Registry::with_builtins()),
        Config::default(),
    ));
    let ws = state.workspaces.open(&root).await.unwrap();
    let journal = FileJournal::new(root.clone(), Arc::new(DbFileJournal(ws.db.clone())));
    let path = root.join("conflict");
    journal
        .prepare(
            &path,
            Some(b"planned"),
            FileOrigin {
                run_id: Uuid::new_v4(),
                node_id: Uuid::new_v4(),
                attempt: 1,
                wal_position: 0,
            },
            None,
        )
        .unwrap();
    std::fs::write(&path, b"user").unwrap();
    let service = DaemonService::new(state);
    let request = || {
        Request::new(SendChatRequest {
            workspace_path: root.to_string_lossy().into(),
            message: "test".into(),
            options_json: serde_json::json!({"provider":"mock", "mock_response":"ready"})
                .to_string(),
            ..Default::default()
        })
    };
    let error = Daemon::send_chat(&service, request()).await.err().unwrap();
    assert_eq!(error.code(), tonic::Code::FailedPrecondition);
    assert!(error.message().contains("file recovery conflicts"));
    assert_eq!(std::fs::read(&path).unwrap(), b"user");
    std::fs::remove_file(&path).unwrap();
    let mut events = Daemon::send_chat(&service, request()).await.unwrap().into_inner();
    let mut done = false;
    while let Some(event) = events.next().await {
        let event = event.unwrap();
        assert_ne!(event.kind, "error", "{}", event.content);
        done |= event.kind == "done";
    }
    assert!(done);
}

#[tokio::test]
async fn unreadable_chat_checkpoint_stops_the_turn_before_tool_effects() {
    let root = std::env::temp_dir().join(format!("metteur-chat-recovery-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let state = Arc::new(AppState::new(
        WorkspaceManager::new().with_global_config_path(root.join(".metteur/global.toml")),
        Arc::new(Registry::with_builtins()),
        Config::default(),
    ));
    let ws = state.workspaces.open(&root).await.unwrap();
    ws.db.put(cf::CHAT_CHECKPOINTS, Uuid::new_v4().as_bytes(), b"unreadable").unwrap();
    let service = DaemonService::new(state);
    let request = Request::new(SendChatRequest {
        workspace_path: root.to_string_lossy().into(), message: "write".into(),
        options_json: serde_json::json!({"provider":"mock", "permission_mode":"sandbox", "mock_steps":[
            {"tool_calls":[{"name":"WriteFile", "arguments":{"path":"never", "content":"no"}}]}, {"text":"done"}
        ]}).to_string(), ..Default::default()
    });
    let error = Daemon::send_chat(&service, request).await.err().unwrap();
    assert_eq!(error.code(), tonic::Code::FailedPrecondition);
    assert!(error.message().contains("no new turn started"), "unexpected admission error: {error}");
    assert!(!root.join("never").exists());
}
