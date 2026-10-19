use metteur_daemon::{project_memory::{ProjectMemory, NoteInput, MemoryQuery}, storage::persistence::Db};
use metteur_shared::resources::{ResourceRole, Origin, Validity};
use uuid::Uuid;
fn setup() -> (std::path::PathBuf, Db, ProjectMemory) {
    let root = std::env::temp_dir().join(format!("nx-memory-{}", Uuid::new_v4()));
    std::fs::create_dir_all(root.join(".metteur")).unwrap();
    std::fs::write(root.join("source.rs"), "original").unwrap();
    let db = Db::open(&root.join(".metteur/db")).unwrap();
    let service = ProjectMemory::new(db.clone(), &root).unwrap();
    (root, db, service)
}
#[tokio::test]
async fn scoped_revisions_confirmation_invalidation_and_tombstone() {
    let (root, db, service) = setup();
    let (_, _, other) = setup();
    let id = Uuid::new_v4();
    let note = NoteInput { id, title: "Review".into(), body: "I think validation passed. api_key=synthetic-secret-only".into(), files: vec!["source.rs".into()] };
    let entry = service.write_note(ResourceRole::MainAgent, "agent", None, Uuid::new_v4(), 0, note.clone()).await.unwrap();
    assert_eq!(entry.content.origin, Origin::ModelOpinion);
    assert!(!entry.content.body.contains("synthetic-secret-only"));
    assert!(other.read(id).is_err());
    assert!(service.promote(ResourceRole::MainAgent, "agent", id, 1, &entry.reference.content_digest, Uuid::new_v4()).is_err());
    assert!(service.promote(ResourceRole::User, "local", id, 1, "forged", Uuid::new_v4()).is_err());
    let request = Uuid::new_v4();
    let promoted = service.promote(ResourceRole::User, "local", id, 1, &entry.reference.content_digest, request).unwrap();
    assert_eq!(promoted.content.origin, Origin::UserConvention);
    assert_eq!(service.promote(ResourceRole::User, "local", id, 1, &entry.reference.content_digest, request).unwrap().reference.revision, 2);
    assert!(service.confirmed_conventions().await.unwrap().contains("I think"));
    std::fs::write(root.join("source.rs"), "changed").unwrap();
    assert_eq!(service.read(id).unwrap().content.validity, Validity::NeedsReview);
    assert!(service.confirmed_conventions().await.unwrap().is_empty());
    assert!(service.write_note(ResourceRole::MainAgent, "agent", None, Uuid::new_v4(), 1, note.clone()).await.is_err());
    let edited = service.write_note(ResourceRole::MainAgent, "agent", None, Uuid::new_v4(), 2, note).await.unwrap();
    assert!(edited.content.confirmation.is_none());
    let removed = service.set_status(ResourceRole::User, id, 3, Uuid::new_v4(), Validity::Tombstone).unwrap();
    assert_eq!(removed.content.validity, Validity::Tombstone);
    drop(service);
    assert_eq!(ProjectMemory::new(db, &root).unwrap().read(id).unwrap().reference.revision, 4);
}
#[tokio::test]
async fn query_limits_strict_inputs_and_role_denials() {
    let (_, _, service) = setup();
    for field in ["origin", "author", "validation_passed", "run_id", "scope", "confirmation"] {
        let mut value = serde_json::json!({"id":Uuid::new_v4(),"title":"t","body":"b"}); value[field] = true.into();
        assert!(serde_json::from_value::<NoteInput>(value).is_err());
    }
    let note = NoteInput { id: Uuid::new_v4(), title:"t".into(), body:"b".into(), files: vec![] };
    for role in [ResourceRole::Supervisor, ResourceRole::Concierge, ResourceRole::ReadOnlyAgent] {
        assert_eq!(service.write_note(role, "untrusted", None, Uuid::new_v4(), 0, note.clone()).await.unwrap_err().code, "permission_denied");
    }
    assert!(service.query(MemoryQuery { keyword: "x".repeat(257), ..Default::default() }).is_err());
    for _ in 0..3 { service.write_note(ResourceRole::MainAgent, "agent", None, Uuid::new_v4(), 0, NoteInput { id: Uuid::new_v4(), ..note.clone() }).await.unwrap(); }
    let page = service.query(MemoryQuery { limit: 2, ..Default::default() }).unwrap();
    assert_eq!(page.entries.len(), 2); assert!(page.next.is_some());
    assert_eq!(service.query(MemoryQuery { after: page.next, ..Default::default() }).unwrap().entries.len(), 1);
    assert!(service.evidence(Uuid::new_v4(), None).await.is_err());
    assert!(service.record_handoff(Uuid::new_v4(), Uuid::new_v4(), "Unbound summary").await.is_err());
}
#[tokio::test]
async fn rpc_controls_bind_workspace_and_reject_forged_fields() {
    use metteur_daemon::{AppState, DaemonService, Registry, WorkspaceManager, grpc::proto::{daemon_server::Daemon, ResourceRequest}};
    use metteur_shared::config::Config;
    use std::sync::Arc;
    let root = std::env::temp_dir().join(format!("nx-memory-rpc-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let manager = WorkspaceManager::new().with_global_config_path(root.join("isolated-global.toml"));
    manager.open(&root).await.unwrap();
    let service = DaemonService::new(Arc::new(AppState::new(manager, Arc::new(Registry::with_builtins()), Config::default())));
    let id = Uuid::new_v4();
    let request = ResourceRequest { schema_version:1, request_id:Uuid::new_v4().to_string(), workspace_path:root.to_string_lossy().into(), expected_revision:0, payload_json:serde_json::json!({"id":id,"title":"RPC note","body":"user text","origin":"execution_evidence"}).to_string() };
    assert!(service.write_project_note(tonic::Request::new(request.clone())).await.unwrap().into_inner().error_json.contains("invalid_request"));
    let mut request = request; request.payload_json = serde_json::json!({"id":id,"title":"RPC note","body":"user text"}).to_string();
    let result = service.write_project_note(tonic::Request::new(request)).await.unwrap().into_inner();
    assert!(result.error_json.is_empty(), "{}", result.error_json);
    let value: serde_json::Value = serde_json::from_str(&result.payload_json).unwrap();
    assert_eq!(value["content"]["origin"], "user_note");
    assert_eq!(value["content"]["author"], "local");
}
