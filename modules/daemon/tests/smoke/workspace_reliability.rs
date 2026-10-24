use super::*;

#[tokio::test]
async fn file_creation_and_save_reject_stale_overwrites() {
    let (mut client, root) = start_server(metteur_shared::config::Config::default()).await;
    let ws = root.to_string_lossy().into_owned();
    client
        .open_workspace(OpenWorkspaceRequest {
            path: ws.clone(),
        })
        .await
        .unwrap();
    let path = "nested/稿件.txt".to_string();
    client
        .write_file(WriteFileRequest {
            workspace_path: ws.clone(),
            path: path.clone(),
            content: "original".into(),
            create_only: true,
            ..Default::default()
        })
        .await
        .unwrap();
    let exists = client
        .write_file(WriteFileRequest {
            workspace_path: ws.clone(),
            path: path.clone(),
            content: "must not replace".into(),
            create_only: true,
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(exists.code(), tonic::Code::AlreadyExists);
    std::fs::write(root.join(&path), "external edit").unwrap();
    let stale = client
        .write_file(WriteFileRequest {
            workspace_path: ws.clone(),
            path: path.clone(),
            content: "stale local draft".into(),
            expected_content: Some("original".into()),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(stale.code(), tonic::Code::FailedPrecondition);
    assert_eq!(std::fs::read_to_string(root.join(&path)).unwrap(), "external edit");
    client
        .write_file(WriteFileRequest {
            workspace_path: ws.clone(),
            path: path.clone(),
            content: "reviewed save".into(),
            expected_content: Some("external edit".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(std::fs::read_to_string(root.join(&path)).unwrap(), "reviewed save");
    let files = client
        .list_files(proto::ListFilesRequest {
            workspace_path: ws.clone(),
            dir: "nested".into(),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(
        files.entries.iter().any(|entry| entry.path == "nested/稿件.txt"),
        "All platforms expose slash-separated relative paths"
    );
    client
        .remove_file(RemoveFileRequest {
            workspace_path: ws.clone(),
            path: path.clone(),
        })
        .await
        .unwrap();
    client
        .write_file(WriteFileRequest {
            workspace_path: ws,
            path: path.clone(),
            content: "restored buffer".into(),
            create_only: true,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(std::fs::read_to_string(root.join(path)).unwrap(), "restored buffer");
}

#[tokio::test]
async fn idle_subscriptions_do_not_keep_the_workspace_locked_after_close() {
    let (mut client, root) = start_server(metteur_shared::config::Config::default()).await;
    let ws = root.to_string_lossy().into_owned();
    client
        .open_workspace(OpenWorkspaceRequest {
            path: ws.clone(),
        })
        .await
        .unwrap();
    let request = proto::WatchWorkspaceRequest {
        workspace_path: ws.clone(),
        ..Default::default()
    };
    let mut changes = client.watch_workspace_changes(request.clone()).await.unwrap().into_inner();
    changes.message().await.unwrap().unwrap();
    let mut files = client.watch_workspace(request).await.unwrap().into_inner();
    let mut jobs = client
        .watch_jobs(proto::WatchJobsRequest {
            workspace_path: ws.clone(),
        })
        .await
        .unwrap()
        .into_inner();
    client
        .close_workspace(CloseWorkspaceRequest {
            path: ws.clone(),
        })
        .await
        .unwrap();
    client
        .open_workspace(OpenWorkspaceRequest {
            path: ws.clone(),
        })
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), changes.message())
            .await
            .unwrap()
            .unwrap()
            .is_none()
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(2), files.message())
            .await
            .unwrap()
            .unwrap()
            .is_none()
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(2), jobs.message())
            .await
            .unwrap()
            .unwrap()
            .is_none()
    );
    client
        .close_workspace(CloseWorkspaceRequest {
            path: ws,
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn workspace_stream_pushes_committed_changes_and_is_idle_without_changes() {
    let (mut client, root) = start_server(metteur_shared::config::Config::default()).await;
    let ws = root.to_string_lossy().into_owned();
    client
        .open_workspace(OpenWorkspaceRequest {
            path: ws.clone(),
        })
        .await
        .unwrap();
    let mut stream = client
        .watch_workspace_changes(proto::WatchWorkspaceRequest {
            workspace_path: ws.clone(),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    let first = tokio::time::timeout(Duration::from_secs(2), stream.message())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .sequence;
    assert!(
        tokio::time::timeout(Duration::from_millis(700), stream.message()).await.is_err(),
        "Idle workspaces must not push periodic refreshes"
    );
    let mut chat = client
        .send_chat(SendChatRequest {
            workspace_path: ws.clone(),
            message: "Synthetic push probe".into(),
            options_json:
                serde_json::json!({"provider":"mock","mock_text":"Synthetic completed turn"})
                    .to_string(),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    while chat.message().await.unwrap().is_some() {}
    let changed = tokio::time::timeout(Duration::from_secs(2), stream.message())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(changed.sequence > first);
}
