//! One transport reuses the existing authorized file, business and job sources.
use metteur_daemon::{AppState, DaemonService, Registry, WorkspaceManager};
use metteur_proto::proto::{CloseWorkspaceRequest, WatchWorkspaceRequest, daemon_server::Daemon};
use metteur_shared::config::{AclConfig, AclRule, Config};
use std::sync::Arc;
use tokio_stream::StreamExt;
use tonic::Request;
use uuid::Uuid;

#[tokio::test]
async fn shared_activity_delivers_real_files_business_and_job_terminals_then_releases_on_close() {
    let root = std::env::temp_dir().join(format!("dw-activity-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let state = Arc::new(AppState::new(
        WorkspaceManager::new().with_global_config_path(root.join("global.toml")),
        Arc::new(Registry::with_builtins()),
        Config::default(),
    ));
    let ws = state.workspaces.open(&root).await.unwrap();
    let service = DaemonService::new(state.clone());
    let mut events = service
        .watch_workspace_changes(Request::new(WatchWorkspaceRequest {
            workspace_path: root.to_string_lossy().into(),
            include_activity: true,
        }))
        .await
        .unwrap()
        .into_inner();
    let first = events.next().await.unwrap().unwrap();
    assert!(first.activity_supported && first.file_event.is_none() && first.job_event.is_none());
    ws.db.notify_changes();
    std::fs::write(root.join("observed.txt"), "actual filesystem event").unwrap();
    let job = ws.jobs().start("echo workspace-activity", &root, Uuid::new_v4(), 4096).unwrap();
    let mut business = false;
    let mut file = false;
    let mut terminal = false;
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while let Some(event) = events.next().await {
            let event = event.unwrap();
            assert!(event.activity_supported);
            if event.file_event.as_ref().is_some_and(|e| e.path == "observed.txt") {
                file = true;
            }
            if event
                .job_event
                .as_ref()
                .is_some_and(|e| e.job_id == job && e.kind == "finished" && e.exit_code == 0)
            {
                terminal = true;
            }
            if event.file_event.is_none()
                && event.job_event.is_none()
                && event.sequence > first.sequence
            {
                business = true;
            }
            if business && file && terminal {
                break;
            }
        }
    })
    .await
    .unwrap();
    assert!(business && file && terminal);
    drop(ws);
    service
        .close_workspace(Request::new(CloseWorkspaceRequest {
            path: root.to_string_lossy().into(),
        }))
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while let Some(event) = events.next().await {
            event.unwrap();
        }
    })
    .await
    .unwrap();
    state.workspaces.open(&root).await.unwrap();
}

#[tokio::test]
async fn combined_subscription_cannot_bypass_the_original_file_or_job_acl() {
    let root = std::env::temp_dir().join(format!("dw-activity-acl-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let state = Arc::new(AppState::new(
        WorkspaceManager::new().with_global_config_path(root.join("global.toml")),
        Arc::new(Registry::with_builtins()),
        Config::default(),
    ));
    state.workspaces.open(&root).await.unwrap();
    *state.acl_store.write().unwrap() = AclConfig {
        rules: vec![AclRule {
            subject: "local".into(),
            allow: vec!["/metteur.Daemon/WatchWorkspaceChanges".into()],
            deny: vec![],
        }],
    };
    let service = DaemonService::new(state);
    let error = service
        .watch_workspace_changes(Request::new(WatchWorkspaceRequest {
            workspace_path: root.to_string_lossy().into(),
            include_activity: true,
        }))
        .await
        .unwrap_err();
    assert_eq!(error.code(), tonic::Code::PermissionDenied);
    let mut legacy = service
        .watch_workspace_changes(Request::new(WatchWorkspaceRequest {
            workspace_path: root.to_string_lossy().into(),
            include_activity: false,
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(!legacy.next().await.unwrap().unwrap().activity_supported);
}
