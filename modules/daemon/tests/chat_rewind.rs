//! RPC-level regression tests: the next request must use the restored model
//! state, not the discarded transcript or a late running-turn finalizer.
use std::path::{Path, PathBuf};
use std::sync::Arc;

use metteur_daemon::chat::session::load_thread;
use metteur_daemon::{AppState, DaemonService, Registry, WorkspaceManager};
use metteur_proto::proto::daemon_server::Daemon;
use metteur_proto::proto::{RewindChatRequest, SendChatRequest};
use metteur_shared::config::Config;
use tokio_stream::StreamExt;
use tonic::Request;

async fn setup() -> (DaemonService, Arc<AppState>, PathBuf) {
    let root = std::env::temp_dir().join(format!("metteur-rewind-rpc-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("code.txt"), "before").unwrap();
    let state = Arc::new(AppState::new(
        WorkspaceManager::new().with_global_config_path(root.join(".metteur/global.toml")),
        Arc::new(Registry::with_builtins()),
        Config::default(),
    ));
    state.workspaces.open(&root).await.unwrap();
    (DaemonService::new(state.clone()), state, root)
}

fn request(
    root: &Path,
    session: &str,
    message: &str,
    options: serde_json::Value,
) -> Request<SendChatRequest> {
    Request::new(SendChatRequest {
        workspace_path: root.to_string_lossy().into(),
        session_id: session.into(),
        message: message.into(),
        history_json: "[]".into(),
        options_json: options.to_string(),
    })
}

async fn turn(
    service: &DaemonService,
    root: &Path,
    session: &str,
    message: &str,
    options: serde_json::Value,
) -> (String, String) {
    let mut stream = Daemon::send_chat(service, request(root, session, message, options))
        .await
        .unwrap()
        .into_inner();
    let mut ids = (String::new(), String::new());
    while let Some(event) = stream.next().await {
        let event = event.unwrap();
        assert_ne!(event.kind, "error", "{}", event.content);
        if event.kind == "session" {
            let detail: serde_json::Value = serde_json::from_str(&event.detail_json).unwrap();
            ids = (
                detail["session_id"].as_str().unwrap().into(),
                detail["checkpoint_id"].as_str().unwrap().into(),
            );
        }
    }
    assert!(!ids.1.is_empty());
    ids
}

#[tokio::test]
async fn rewind_restores_files_and_authoritative_context_before_the_next_request() {
    let (service, state, root) = setup().await;
    let options = serde_json::json!({"provider":"mock", "mock_steps":[
        {"tool_calls":[{"name":"TodoWrite","arguments":{"todos":[
            {"content":"keep earlier task","status":"pending","active_form":"Planning"}
        ]}}]}, {"text":"kept answer"}
    ]});
    let (session, first_checkpoint) = turn(&service, &root, "", "keep", options.clone()).await;
    let ws = state.workspaces.get(&root).await.unwrap();
    let id = uuid::Uuid::parse_str(&session).unwrap();
    let before = load_thread(&ws.db, &id).unwrap().unwrap();
    assert_eq!(before.todos.len(), 1);
    let (_, checkpoint) = turn(&service, &root, &session, "discarded request", serde_json::json!({
        "provider":"mock", "permission_mode":"sandbox", "mock_steps":[
            {"tool_calls":[{"name":"WriteFile","arguments":{"path":"code.txt","content":"discarded content"}}]},
            {"text":"discarded answer"}
        ]
    })).await;
    assert_eq!(std::fs::read_to_string(root.join("code.txt")).unwrap(), "discarded content");
    let response = Daemon::rewind_chat(
        &service,
        Request::new(RewindChatRequest {
            workspace_path: root.to_string_lossy().into(),
            session_id: session.clone(),
            snapshot_id: checkpoint,
        }),
    )
    .await
    .unwrap()
    .into_inner();
    assert!(!response.transcript_json.contains("discarded"));
    assert_eq!(std::fs::read_to_string(root.join("code.txt")).unwrap(), "before");
    let restored = load_thread(&ws.db, &id).unwrap().unwrap();
    assert_eq!(
        serde_json::to_value(restored.context).unwrap(),
        serde_json::to_value(before.context).unwrap()
    );
    turn(
        &service,
        &root,
        &session,
        "continue after rewind",
        serde_json::json!({"provider":"mock", "mock_text":"continued"}),
    )
    .await;
    let continued = load_thread(&ws.db, &id).unwrap().unwrap();
    assert_eq!(continued.todos, before.todos);
    assert!(!serde_json::to_string(&continued).unwrap().contains("discarded"));
    // The first checkpoint is still reachable. Rewinding it gives an empty
    // session, not a new id seeded with the discarded frontend history.
    Daemon::rewind_chat(
        &service,
        Request::new(RewindChatRequest {
            workspace_path: root.to_string_lossy().into(),
            session_id: session,
            snapshot_id: first_checkpoint,
        }),
    )
    .await
    .unwrap();
    let empty = load_thread(&ws.db, &id).unwrap().unwrap();
    assert!(empty.context.messages.is_empty());
    assert!(empty.transcript.is_empty());
    assert_eq!(empty.turns, 0);
}

#[tokio::test]
async fn rewind_waits_for_active_turn_and_late_finalize_cannot_resurrect_it() {
    let (service, state, root) = setup().await;
    let mut stream = Daemon::send_chat(
        &service,
        request(
            &root,
            "",
            "discard active turn",
            serde_json::json!({
                "provider":"mock", "mock_text":"late answer", "mock_delay_ms":1000
            }),
        ),
    )
    .await
    .unwrap()
    .into_inner();
    let event = stream.next().await.unwrap().unwrap();
    assert_eq!(event.kind, "session");
    let detail: serde_json::Value = serde_json::from_str(&event.detail_json).unwrap();
    let session = detail["session_id"].as_str().unwrap();
    let checkpoint = detail["checkpoint_id"].as_str().unwrap();
    let blueprint =
        metteur_shared::dsl::compile("blueprint \"guard\"\nentry start: Start()\n").unwrap();
    let error = Daemon::execute_blueprint(
        &service,
        Request::new(metteur_proto::proto::ExecuteBlueprintRequest {
            workspace_path: root.to_string_lossy().into(),
            blueprint_id: blueprint.id.to_string(),
            blueprint_json: serde_json::to_string(&blueprint).unwrap(),
        }),
    )
    .await
    .expect_err("a blueprint must not enter a running chat's workspace");
    assert_eq!(error.code(), tonic::Code::FailedPrecondition);
    Daemon::rewind_chat(
        &service,
        Request::new(RewindChatRequest {
            workspace_path: root.to_string_lossy().into(),
            session_id: session.into(),
            snapshot_id: checkpoint.into(),
        }),
    )
    .await
    .unwrap();
    while stream.next().await.is_some() {}
    let ws = state.workspaces.get(&root).await.unwrap();
    let record = load_thread(&ws.db, &uuid::Uuid::parse_str(session).unwrap()).unwrap().unwrap();
    assert!(record.context.messages.is_empty());
    assert!(record.transcript.is_empty());
}
