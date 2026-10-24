//! Real file, Version Flow and RPC boundaries for the daily file workflow.
use metteur_daemon::execution::file_journal::{DbFileJournal, FileJournal, FileOrigin};
use metteur_daemon::storage::{persistence::Db, resources::digest, versioning::VersionManager};
use metteur_daemon::{AppState, DaemonService, Registry, WorkspaceManager};
use metteur_proto::proto::{ReadFileRequest, WriteFileRequest, daemon_server::Daemon};
use metteur_shared::config::Config;
use std::{path::PathBuf, sync::Arc};
use tonic::Request;
use uuid::Uuid;

fn root() -> PathBuf {
    let root = std::env::temp_dir().join(format!("dw-files-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    root
}
#[test]
fn frozen_file_keeps_exact_text_and_reuses_history_without_trusting_client_digest() {
    let root = root();
    let db = Db::open(&root.join(".metteur/db")).unwrap();
    let versions = VersionManager::new(db, root.clone());
    let raw = "\u{feff}first 中文\r\nsecond\r\n";
    std::fs::write(root.join("a.txt"), raw).unwrap();
    let first = versions.capture_file("a.txt").unwrap();
    let count = versions.list_snapshots().unwrap().len();
    assert_eq!(versions.capture_file("a.txt").unwrap(), first);
    assert_eq!(versions.list_snapshots().unwrap().len(), count);
    std::fs::write(root.join("a.txt"), "new disk").unwrap();
    assert_eq!(
        versions.referenced_file(first.snapshot_id, "a.txt", &digest(raw.as_bytes())).unwrap(),
        raw
    );
    assert!(versions.referenced_file(first.snapshot_id, "a.txt", &digest(b"new disk")).is_err());
    assert!(
        versions.referenced_file(first.snapshot_id, "../a.txt", &digest(raw.as_bytes())).is_err()
    );
    let other = root.join("other");
    std::fs::create_dir(&other).unwrap();
    let foreign = VersionManager::new(Db::open(&other.join(".metteur/db")).unwrap(), other);
    assert!(foreign.referenced_file(first.snapshot_id, "a.txt", &digest(raw.as_bytes())).is_err());
}
#[test]
fn create_only_refuses_a_file_appearing_after_preparation() {
    let root = root();
    let db = Db::open(&root.join(".metteur/db")).unwrap();
    let journal = FileJournal::new(root.clone(), Arc::new(DbFileJournal(db)));
    let path = root.join("copy.txt");
    let origin = FileOrigin {
        run_id: Uuid::nil(),
        node_id: Uuid::nil(),
        attempt: 0,
        wal_position: 0,
    };
    let mut operation = journal.prepare_new(&path, b"local", origin.clone()).unwrap();
    std::fs::write(&path, b"external").unwrap();
    assert!(journal.apply(&mut operation).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"external");
    assert!(journal.prepare_new(&path, b"local", origin).is_err());
}
#[tokio::test]
async fn file_rpc_freezes_versions_rejects_large_binary_and_conditions_blueprint_saves() {
    let root = root();
    std::fs::write(root.join("text.txt"), "\u{feff}saved").unwrap();
    let manager = WorkspaceManager::new().with_global_config_path(root.join("global.toml"));
    manager.open(&root).await.unwrap();
    let service = DaemonService::new(Arc::new(AppState::new(
        manager,
        Arc::new(Registry::with_builtins()),
        Config::default(),
    )));
    let request = |path: &str| ReadFileRequest {
        workspace_path: root.to_string_lossy().into(),
        path: path.into(),
        freeze_reference: true,
    };
    let frozen = service.read_file(Request::new(request("text.txt"))).await.unwrap().into_inner();
    let reference: serde_json::Value = serde_json::from_str(&frozen.file_reference_json).unwrap();
    assert_eq!(reference["digest"], digest("\u{feff}saved".as_bytes()));
    assert_eq!(frozen.content_utf8.as_deref(), Some("\u{feff}saved".as_bytes()));
    assert_eq!(reference["path"], "text.txt");
    std::fs::write(root.join("large.txt"), vec![b'a'; 65537]).unwrap();
    assert_eq!(
        service.read_file(Request::new(request("large.txt"))).await.unwrap_err().code(),
        tonic::Code::ResourceExhausted
    );
    std::fs::write(root.join("binary.txt"), [0xff]).unwrap();
    assert_eq!(
        service.read_file(Request::new(request("binary.txt"))).await.unwrap_err().code(),
        tonic::Code::InvalidArgument
    );
    let graph =
        metteur_shared::dsl::compile("blueprint \"file CAS\"\nentry s: Start\ne: End\ns -> e\n")
            .unwrap();
    let original = serde_json::to_string(&graph).unwrap();
    let mut write = WriteFileRequest {
        workspace_path: root.to_string_lossy().into(),
        path: "plan.blueprint".into(),
        content: original.clone(),
        create_only: true,
        expected_content: None,
    };
    service.write_file(Request::new(write.clone())).await.unwrap();
    let mut external = graph.clone();
    external.name = "external".into();
    let external = serde_json::to_string(&external).unwrap();
    std::fs::write(root.join("plan.blueprint"), &external).unwrap();
    write.create_only = false;
    write.expected_content = Some(original);
    assert!(service.write_file(Request::new(write.clone())).await.is_err());
    assert_eq!(std::fs::read_to_string(root.join("plan.blueprint")).unwrap(), external);
    write.expected_content = Some(external);
    service.write_file(Request::new(write.clone())).await.unwrap();
    assert_eq!(std::fs::read_to_string(root.join("plan.blueprint")).unwrap(), write.content);
}
