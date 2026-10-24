//! Actual daemon adapters, workspace files and RocksDB; no model or UI mock.
use metteur_daemon::{
    AppState, DaemonService, Registry, WorkspaceManager, storage::persistence::Db,
};
use metteur_proto::proto::{
    RenameFileRequest, ResourceRequest, ResourceResponse, daemon_server::Daemon,
};
use metteur_shared::config::Config;
use std::sync::Arc;
use tonic::Request;
use uuid::Uuid;

async fn setup() -> (std::path::PathBuf, DaemonService) {
    let root = std::env::temp_dir().join(format!("dw-recovery-rpc-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), "disk").unwrap();
    let manager = WorkspaceManager::new().with_global_config_path(root.join("global.toml"));
    manager.open(&root).await.unwrap();
    let mut state = AppState::new(manager, Arc::new(Registry::with_builtins()), Config::default());
    state.global_db = Some(Db::open(&root.join("user-db")).unwrap());
    (root, DaemonService::new(Arc::new(state)))
}
fn req(
    root: &std::path::Path,
    scope: &str,
    action: serde_json::Value,
    revision: u64,
) -> ResourceRequest {
    ResourceRequest {
        schema_version: 1,
        request_id: Uuid::new_v4().to_string(),
        workspace_path: root.to_string_lossy().into(),
        expected_revision: revision,
        payload_json: serde_json::json!({"scope":scope,"action":action}).to_string(),
    }
}
fn body(reply: ResourceResponse) -> serde_json::Value {
    assert!(reply.error_json.is_empty(), "{}", reply.error_json);
    serde_json::from_str(&reply.payload_json).unwrap()
}
fn draft(id: Uuid, entity: &str) -> serde_json::Value {
    serde_json::json!({"id":id,"window_id":Uuid::new_v4(),"kind":"text","entity":entity,"edit_sequence":1,"raw":"unparsed {\r\n中文","baseline_identity":"disk","selections":[{"scope":"personal"},{"scope":"project"},{"scope":"personal"}],"pending_submission":Uuid::new_v4(),"selected_model":"fixture-model","view":{"cursor":9}})
}

#[tokio::test]
async fn recovery_rpc_preserves_raw_bytes_and_never_writes_or_recreates_sources() {
    let (root, service) = setup().await;
    let id = Uuid::new_v4();
    let input = draft(id, "a.txt");
    let command = req(&root, "project", serde_json::json!({"operation":"backup","input":input}), 0);
    let accepted =
        body(service.recovery(Request::new(command.clone())).await.unwrap().into_inner());
    assert_eq!(accepted["revision"], 1);
    assert_eq!(accepted["input"], input);
    let replay = body(service.recovery(Request::new(command)).await.unwrap().into_inner());
    assert_eq!(replay["revision"], 1);
    assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "disk");
    std::fs::remove_file(root.join("a.txt")).unwrap();
    let recovered = body(
        service
            .recovery(Request::new(req(
                &root,
                "project",
                serde_json::json!({"operation":"read","id":id}),
                0,
            )))
            .await
            .unwrap()
            .into_inner(),
    );
    assert_eq!(recovered["record"]["input"]["raw"], input["raw"]);
    assert_eq!(recovered["source_available"], false);
    assert_eq!(recovered["restores_permissions"], false);
    assert!(!root.join("a.txt").exists());
}

#[tokio::test]
async fn latest_view_rpc_returns_only_workspace_metadata_without_starting_or_writing_files() {
    let (root, service) = setup().await;
    let empty = body(
        service
            .recovery(Request::new(req(
                &root,
                "project",
                serde_json::json!({"operation":"latest_view"}),
                0,
            )))
            .await
            .unwrap()
            .into_inner(),
    );
    assert!(empty["record"].is_null());
    let mut input = draft(Uuid::new_v4(), "workspace");
    input["kind"] = "view_state".into();
    input["raw"] = "".into();
    input["selections"] = serde_json::json!([]);
    input["pending_submission"] = serde_json::Value::Null;
    input["selected_model"] = serde_json::Value::Null;
    input["view"] = serde_json::json!({"schema":1,"tabs":[{"file":"a.txt"}],"active":"file:a.txt","session":Uuid::new_v4(),"positions":{"a.txt":{"kind":"text","selections":[{"anchor":1,"head":2}],"top":10,"left":0}}});
    body(
        service
            .recovery(Request::new(req(
                &root,
                "project",
                serde_json::json!({"operation":"backup","input":input}),
                0,
            )))
            .await
            .unwrap()
            .into_inner(),
    );
    let loaded = body(
        service
            .recovery(Request::new(req(
                &root,
                "project",
                serde_json::json!({"operation":"latest_view"}),
                0,
            )))
            .await
            .unwrap()
            .into_inner(),
    );
    assert_eq!(loaded["record"]["input"], input);
    assert_eq!(loaded["restores_permissions"], false);
    assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "disk");
    assert_eq!(
        std::fs::read_dir(&root)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "txt"))
            .count(),
        1
    );
}

#[tokio::test]
async fn recovery_rpc_rejects_forged_owner_scope_path_schema_and_authority() {
    let (root, service) = setup().await;
    for (scope, input) in [
        ("project", draft(Uuid::new_v4(), "../escape")),
        ("personal", draft(Uuid::new_v4(), "a.txt")),
        ("project", draft(Uuid::new_v4(), ".metteur/private")),
    ] {
        let reply = service
            .recovery(Request::new(req(
                &root,
                scope,
                serde_json::json!({"operation":"backup","input":input}),
                0,
            )))
            .await
            .unwrap()
            .into_inner();
        assert!(!reply.error_json.is_empty());
    }
    for field in ["owner", "permission_mode", "allow_once", "approved"] {
        let mut input = draft(Uuid::new_v4(), "a.txt");
        input[field] = "forged".into();
        let reply = service
            .recovery(Request::new(req(
                &root,
                "project",
                serde_json::json!({"operation":"backup","input":input}),
                0,
            )))
            .await
            .unwrap()
            .into_inner();
        assert!(reply.error_json.contains("invalid_request"));
    }
    let mut request = req(&root, "project", serde_json::json!({"operation":"list"}), 0);
    request.schema_version = 999;
    assert!(
        service
            .recovery(Request::new(request))
            .await
            .unwrap()
            .into_inner()
            .error_json
            .contains("unsupported_request")
    );
}

#[tokio::test]
async fn confirmed_file_rename_transfers_drafts_without_overwriting_them() {
    let (root, service) = setup().await;
    let id = Uuid::new_v4();
    body(
        service
            .recovery(Request::new(req(
                &root,
                "project",
                serde_json::json!({"operation":"backup","input":draft(id,"a.txt")}),
                0,
            )))
            .await
            .unwrap()
            .into_inner(),
    );
    service
        .rename_file(Request::new(RenameFileRequest {
            workspace_path: root.to_string_lossy().into(),
            from: "a.txt".into(),
            to: "renamed.txt".into(),
        }))
        .await
        .unwrap();
    assert_eq!(std::fs::read_to_string(root.join("renamed.txt")).unwrap(), "disk");
    let record = body(
        service
            .recovery(Request::new(req(
                &root,
                "project",
                serde_json::json!({"operation":"read","id":id}),
                0,
            )))
            .await
            .unwrap()
            .into_inner(),
    );
    assert_eq!(record["record"]["input"]["entity"], "renamed.txt");
    assert_eq!(record["record"]["previous_entity"], "a.txt");
    assert_eq!(record["record"]["input"]["raw"], "unparsed {\r\n中文");
}
