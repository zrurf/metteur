use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use metteur_daemon::{AppState, DaemonService, Registry, WorkspaceManager};
use metteur_proto::proto::daemon_server::Daemon;
use metteur_proto::proto::{AbortChatRequest, ApprovalDecisionRequest, SendChatRequest};
use metteur_shared::config::Config;
use tokio_stream::StreamExt;
use tonic::Request;

async fn setup() -> (DaemonService, Arc<AppState>, PathBuf) {
    let root = super::common::root("rpc");
    let state = Arc::new(
        AppState::new(
            WorkspaceManager::new().with_global_config_path(root.join(".metteur/global.toml")),
            Arc::new(Registry::with_builtins()),
            Config::default(),
        )
        .with_global_db(super::common::db("rpc-global")),
    );
    state.workspaces.open(&root).await.unwrap();
    (DaemonService::new(state.clone()), state, root)
}

fn request(root: &Path, count: usize) -> Request<SendChatRequest> {
    let mut steps: Vec<_> = (0..count).map(|index| serde_json::json!({"tool_calls":[{
        "name":"WriteFile", "arguments":{"path":"file.txt", "content":format!("write {index}")}
    }]})).collect();
    steps.push(serde_json::json!({"text":"done"}));
    Request::new(SendChatRequest {
        workspace_path: root.to_string_lossy().into(),
        session_id: String::new(),
        message: "write the file".into(),
        history_json: "[]".into(),
        options_json:
            serde_json::json!({"provider":"mock", "permission_mode":"ask", "mock_steps":steps})
                .to_string(),
    })
}

fn response(root: &Path, id: &str, decision: &str) -> Request<ApprovalDecisionRequest> {
    Request::new(ApprovalDecisionRequest {
        workspace_path: root.to_string_lossy().into(),
        request_id: id.into(),
        decision: decision.into(),
        ..Default::default()
    })
}

#[tokio::test]
async fn chat_once_choices_reprompt_and_duplicate_responses_cannot_escalate() {
    let (service, state, root) = setup().await;
    let mut stream = Daemon::send_chat(&service, request(&root, 3)).await.unwrap().into_inner();
    let mut approvals = Vec::new();
    while let Some(event) =
        tokio::time::timeout(Duration::from_secs(10), stream.next()).await.unwrap()
    {
        let event = event.unwrap();
        assert_ne!(event.kind, "error", "{}", event.content);
        if event.kind != "approval" {
            continue;
        }
        let detail: serde_json::Value = serde_json::from_str(&event.detail_json).unwrap();
        let id = detail["request_id"].as_str().unwrap();
        let invalid = Daemon::respond_approval(&service, response(&root, id, "AllowForever"))
            .await
            .unwrap_err();
        assert_eq!(invalid.code(), tonic::Code::InvalidArgument);
        let decision = if approvals.is_empty() {
            "AllowOnce"
        } else {
            "DenyOnce"
        };
        Daemon::respond_approval(&service, response(&root, id, decision)).await.unwrap();
        assert!(
            Daemon::respond_approval(&service, response(&root, id, "AllowGlobal")).await.is_err()
        );
        approvals.push(id.to_string());
    }
    assert_eq!(approvals.len(), 3);
    assert_ne!(approvals[0], approvals[1]);
    assert_ne!(approvals[1], approvals[2]);
    assert_eq!(std::fs::read_to_string(root.join("file.txt")).unwrap(), "write 0");
    let hash = metteur_daemon::sandbox::command_hash("file.txt");
    let ws = state.workspaces.get(&root).await.unwrap();
    let grants = metteur_daemon::sandbox::grant::GrantStore::new(
        Some(ws.db.clone()),
        state.global_db.clone(),
    );
    assert_eq!(grants.lookup(hash), None);
    assert!(
        Daemon::respond_approval(&service, response(&root, &approvals[0], "AllowWorkspace"))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn abort_and_disconnect_invalidate_chat_approvals() {
    for disconnect in [false, true] {
        let (service, state, root) = setup().await;
        let mut stream = Daemon::send_chat(&service, request(&root, 1)).await.unwrap().into_inner();
        let id = loop {
            let event = tokio::time::timeout(Duration::from_secs(10), stream.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            if event.kind == "approval" {
                let detail: serde_json::Value = serde_json::from_str(&event.detail_json).unwrap();
                break detail["request_id"].as_str().unwrap().to_string();
            }
        };
        let ws = state.workspaces.get(&root).await.unwrap();
        if disconnect {
            drop(stream);
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    match Daemon::send_chat(&service, request(&root, 0)).await {
                        Ok(next) => {
                            let mut stream = next.into_inner();
                            while let Some(event) = stream.next().await {
                                event.unwrap();
                            }
                            break;
                        }
                        Err(error) => assert_eq!(error.code(), tonic::Code::FailedPrecondition),
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
        } else {
            let response = Daemon::chat_delivery(
                &service,
                Request::new(metteur_proto::proto::ResourceRequest {
                    workspace_path: root.to_string_lossy().into(),
                    schema_version: 1,
                    payload_json: r#"{"operation":"admission"}"#.into(),
                    ..Default::default()
                }),
            )
            .await
            .unwrap()
            .into_inner();
            assert!(response.error_json.is_empty());
            let owner: serde_json::Value = serde_json::from_str(&response.payload_json).unwrap();
            assert_eq!(owner["state"], "chat");
            Daemon::abort_chat(
                &service,
                Request::new(AbortChatRequest {
                    workspace_path: root.to_string_lossy().into(),
                    session_id: owner["session"].as_str().unwrap().into(),
                    turn_id: owner["turn"].as_str().unwrap().into(),
                    ..Default::default()
                }),
            )
            .await
            .unwrap();
        }
        assert!(
            Daemon::respond_approval(&service, response(&root, &id, "AllowGlobal")).await.is_err()
        );
        assert!(!root.join("file.txt").exists());
        let grants = metteur_daemon::sandbox::grant::GrantStore::new(
            Some(ws.db.clone()),
            state.global_db.clone(),
        );
        assert_eq!(grants.lookup(metteur_daemon::sandbox::command_hash("file.txt")), None);
    }
}

#[tokio::test]
async fn chat_run_grants_expire_before_the_next_turn() {
    let (service, _, root) = setup().await;
    for _ in 0..2 {
        let mut stream = Daemon::send_chat(&service, request(&root, 2)).await.unwrap().into_inner();
        let mut approvals = 0;
        while let Some(event) =
            tokio::time::timeout(Duration::from_secs(10), stream.next()).await.unwrap()
        {
            let event = event.unwrap();
            assert_ne!(event.kind, "error", "{}", event.content);
            if event.kind == "approval" {
                approvals += 1;
                let detail: serde_json::Value = serde_json::from_str(&event.detail_json).unwrap();
                Daemon::respond_approval(
                    &service,
                    response(&root, detail["request_id"].as_str().unwrap(), "AllowRun"),
                )
                .await
                .unwrap();
            }
        }
        assert_eq!(approvals, 1);
        assert_eq!(std::fs::read_to_string(root.join("file.txt")).unwrap(), "write 1");
    }
}
