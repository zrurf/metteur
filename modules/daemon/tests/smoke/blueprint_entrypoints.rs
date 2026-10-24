use super::*;
use metteur_daemon::storage::{
    blueprint_files,
    persistence::{Db, cf},
    versioning::VersionManager,
};

fn graph(delay: bool) -> metteur_shared::Blueprint {
    let draft = if delay {
        serde_json::json!({"name":"active file","nodes":{"s":{"kind":"Start"},"d":{"kind":"Delay","Ms":1000},"e":{"kind":"End"}},"flow":["s -> d -> e"]})
    } else {
        serde_json::json!({"name":"saved file","nodes":{"s":{"kind":"Start"},"e":{"kind":"End"}},"flow":["s -> e"]})
    };
    metteur_shared::dsl::compile_draft_value_with_catalog(
        &draft,
        &Registry::with_builtins().authoring_catalog(),
    )
    .unwrap()
}

#[tokio::test]
async fn file_save_inline_assertion_and_restore_share_one_authority() {
    let (mut client, root) = start_server(Default::default()).await;
    let ws = root.to_string_lossy().to_string();
    client
        .open_workspace(OpenWorkspaceRequest {
            path: ws.clone(),
        })
        .await
        .unwrap();
    let graph = graph(false);
    let file = String::from_utf8(blueprint_files::encode_native(&graph).unwrap()).unwrap();
    let unsaved = client
        .execute_blueprint(ExecuteBlueprintRequest {
            workspace_path: ws.clone(),
            blueprint_id: graph.id.to_string(),
            workflow_confirmation: String::new(),
            blueprint_json: file.clone(),
        })
        .await
        .unwrap_err();
    assert_eq!(unsaved.code(), tonic::Code::FailedPrecondition);
    client
        .write_file(WriteFileRequest {
            workspace_path: ws.clone(),
            path: "plan.blueprint".into(),
            content: file.clone(),
            ..Default::default()
        })
        .await
        .unwrap();
    let first = client
        .create_snapshot(CreateSnapshotRequest {
            workspace_path: ws.clone(),
            description: "before rename".into(),
            alias: String::new(),
        })
        .await
        .unwrap()
        .into_inner();
    let mut proto = client
        .load_blueprint(proto::LoadBlueprintRequest {
            workspace_path: ws.clone(),
            blueprint_id: graph.id.to_string(),
        })
        .await
        .unwrap()
        .into_inner();
    proto.name = "changed name".into();
    client
        .save_blueprint(SaveBlueprintRequest {
            workspace_path: ws.clone(),
            blueprint: Some(proto.clone()),
            file_path: String::new(),
            file_json: String::new(),
        })
        .await
        .unwrap();
    let changed = std::fs::read(root.join("plan.blueprint")).unwrap();
    assert_eq!(blueprint_files::decode(&changed).unwrap().name, "changed name");
    let error = client
        .execute_blueprint(ExecuteBlueprintRequest {
            workspace_path: ws.clone(),
            blueprint_id: graph.id.to_string(),
            workflow_confirmation: String::new(),
            blueprint_json: file.clone(),
        })
        .await
        .unwrap_err();
    assert_eq!(error.code(), tonic::Code::FailedPrecondition);
    assert_eq!(std::fs::read(root.join("plan.blueprint")).unwrap(), changed);
    assert!(
        client
            .rename_file(RenameFileRequest {
                workspace_path: ws.clone(),
                from: "plan.blueprint".into(),
                to: "moved.blueprint".into()
            })
            .await
            .is_err()
    );
    client
        .rollback(RollbackRequest {
            workspace_path: ws.clone(),
            snapshot_id: first.id,
            alias: String::new(),
        })
        .await
        .unwrap();
    let restored = client
        .load_blueprint(proto::LoadBlueprintRequest {
            workspace_path: ws.clone(),
            blueprint_id: graph.id.to_string(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(restored.name, graph.name);
    let mut stream = client
        .execute_blueprint(ExecuteBlueprintRequest {
            workspace_path: ws,
            blueprint_id: graph.id.to_string(),
            workflow_confirmation: String::new(),
            blueprint_json: file,
        })
        .await
        .unwrap()
        .into_inner();
    while let Some(event) = stream.message().await.unwrap() {
        assert_ne!(event.kind, "error", "{event:?}");
    }
}

#[tokio::test]
async fn active_save_and_restore_are_rejected_and_external_edits_stop_successors() {
    let (mut client, root) = start_server(Default::default()).await;
    let ws = root.to_string_lossy().to_string();
    client
        .open_workspace(OpenWorkspaceRequest {
            path: ws.clone(),
        })
        .await
        .unwrap();
    let mut graph = graph(true);
    let delay = graph.nodes.iter().find(|n| n.kind == "Delay").unwrap().id.to_string();
    let end = graph.nodes.iter().find(|n| n.kind == "End").unwrap().id.to_string();
    let file = String::from_utf8(blueprint_files::encode_native(&graph).unwrap()).unwrap();
    client
        .write_file(WriteFileRequest {
            workspace_path: ws.clone(),
            path: "active.blueprint".into(),
            content: file.clone(),
            ..Default::default()
        })
        .await
        .unwrap();
    let proto = client
        .load_blueprint(proto::LoadBlueprintRequest {
            workspace_path: ws.clone(),
            blueprint_id: graph.id.to_string(),
        })
        .await
        .unwrap()
        .into_inner();
    let snapshot = client
        .create_snapshot(CreateSnapshotRequest {
            workspace_path: ws.clone(),
            description: "active".into(),
            alias: String::new(),
        })
        .await
        .unwrap()
        .into_inner();
    let mut stream = client
        .execute_blueprint(ExecuteBlueprintRequest {
            workspace_path: ws.clone(),
            blueprint_id: graph.id.to_string(),
            workflow_confirmation: String::new(),
            blueprint_json: String::new(),
        })
        .await
        .unwrap()
        .into_inner();
    loop {
        let event = stream.message().await.unwrap().unwrap();
        if event.kind == "started" && event.node_id == delay {
            break;
        }
    }
    let denied = client
        .save_blueprint(SaveBlueprintRequest {
            workspace_path: ws.clone(),
            blueprint: Some(proto),
            file_path: String::new(),
            file_json: String::new(),
        })
        .await
        .unwrap_err();
    assert_eq!(denied.code(), tonic::Code::FailedPrecondition);
    assert!(
        client
            .write_file(WriteFileRequest {
                workspace_path: ws.clone(),
                path: "active.blueprint".into(),
                content: file.clone(),
                ..Default::default()
            })
            .await
            .is_err()
    );
    assert!(
        client
            .rollback(RollbackRequest {
                workspace_path: ws,
                snapshot_id: snapshot.id,
                alias: String::new()
            })
            .await
            .is_err()
    );
    assert_eq!(std::fs::read_to_string(root.join("active.blueprint")).unwrap(), file);
    graph.name = "external change".into();
    std::fs::write(root.join("active.blueprint"), blueprint_files::encode_native(&graph).unwrap())
        .unwrap();
    let mut failed = false;
    loop {
        match stream.message().await {
            Ok(Some(event)) => {
                assert!(!(event.kind == "started" && event.node_id == end));
                failed |= event.kind == "error";
            }
            Err(_) => {
                failed = true;
                break;
            }
            Ok(None) => break,
        }
    }
    assert!(failed, "external change must stop the old continuation");
}

#[tokio::test]
async fn legacy_database_graph_executes_but_cannot_be_silently_associated_on_save() {
    let (mut client, root) = start_server(Default::default()).await;
    let graph = graph(false);
    {
        let db = Db::open(&root.join(".metteur/db")).unwrap();
        db.put(cf::BLUEPRINTS, graph.id.as_bytes(), &serde_json::to_vec(&graph).unwrap()).unwrap();
    }
    let ws = root.to_string_lossy().to_string();
    client
        .open_workspace(OpenWorkspaceRequest {
            path: ws.clone(),
        })
        .await
        .unwrap();
    let proto = client
        .load_blueprint(proto::LoadBlueprintRequest {
            workspace_path: ws.clone(),
            blueprint_id: graph.id.to_string(),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(
        client
            .save_blueprint(SaveBlueprintRequest {
                workspace_path: ws.clone(),
                blueprint: Some(proto),
                file_path: String::new(),
                file_json: String::new()
            })
            .await
            .is_err()
    );
    assert!(!root.join("blueprints").exists());
    let mut stream = client
        .execute_blueprint(ExecuteBlueprintRequest {
            workspace_path: ws,
            blueprint_id: graph.id.to_string(),
            workflow_confirmation: String::new(),
            blueprint_json: String::new(),
        })
        .await
        .unwrap()
        .into_inner();
    while let Some(event) = stream.message().await.unwrap() {
        assert_ne!(event.kind, "error", "{event:?}");
    }
}

#[tokio::test]
async fn changed_file_cannot_resume_a_checkpoint_with_old_version() {
    use metteur_daemon::execution::{CheckpointSink, DbCheckpointSink, ExecutionCheckpoint};
    let (mut client, root) = start_server(Default::default()).await;
    let mut graph = graph(false);
    let run_id = uuid::Uuid::new_v4();
    {
        let db = Db::open(&root.join(".metteur/db")).unwrap();
        let versions = VersionManager::new(db.clone(), root.clone());
        let base = blueprint_files::save(
            &db,
            &versions,
            &graph,
            "resume.blueprint",
            &blueprint_files::encode_native(&graph).unwrap(),
            None,
        )
        .unwrap();
        let mut cp = ExecutionCheckpoint::running(run_id, graph.id, 1);
        cp.blueprint_version = Some(base);
        cp.pending.push(graph.entry_node_id);
        DbCheckpointSink::new(db, run_id).write(&cp).unwrap();
    }
    graph.name = "restored other content".into();
    std::fs::write(root.join("resume.blueprint"), blueprint_files::encode_native(&graph).unwrap())
        .unwrap();
    let ws = root.to_string_lossy().to_string();
    client
        .open_workspace(OpenWorkspaceRequest {
            path: ws.clone(),
        })
        .await
        .unwrap();
    let error = client
        .continue_execution(ContinueExecutionRequest {
            workspace_path: ws,
            run_id: run_id.to_string(),
        })
        .await
        .unwrap_err();
    assert_eq!(error.code(), tonic::Code::FailedPrecondition);
}
