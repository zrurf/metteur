use metteur_daemon::storage::{persistence::{Db, cf}, resources::ResourceStore};
use metteur_shared::resources::ResourceScope;
use serde_json::json;
use uuid::Uuid;

#[test]
fn additive_migration_cas_replay_restart_and_rollback() {
    let root = std::env::temp_dir().join(format!("nx-store-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let db = Db::open(&root.join("db")).unwrap();
    db.put(cf::CHAT_THREADS, b"legacy", b"untouched").unwrap();
    assert_eq!(ResourceStore::inspect(&db).unwrap(), None);
    ResourceStore::prepare(&db, &root.join("backup")).unwrap();
    ResourceStore::prepare(&db, &root.join("backup")).unwrap();
    let scope = ResourceScope::Project("a".into());
    let store = ResourceStore::new(db.clone(), &scope, "memory").unwrap();
    let id = Uuid::new_v4(); let request = Uuid::new_v4();
    assert_eq!(store.write(id, 0, request, json!({"body":"first"})).unwrap().revision, 1);
    assert_eq!(store.write(id, 0, request, json!({"body":"first"})).unwrap().revision, 1);
    assert_eq!(store.write(id, 0, request, json!({"body":"changed"})).unwrap_err().code, "request_conflict");
    assert_eq!(store.write(id, 0, Uuid::new_v4(), json!({})).unwrap_err().actual_revision, Some(1));
    store.write(id, 1, Uuid::new_v4(), json!({"state":"tombstone"})).unwrap();
    assert!(metteur_daemon::execution::DbCheckpointSink::list(&db).unwrap().is_empty());
    assert_eq!(store.revision(id, 1).unwrap().unwrap().value["body"], "first");
    let other = ResourceStore::new(db.clone(), &ResourceScope::Project("b".into()), "memory").unwrap();
    assert!(other.get(id).unwrap().is_none());
    db.commit_chat_rewind(b"legacy", b"rewound", &[]).unwrap();
    assert_eq!(store.get(id).unwrap().unwrap().revision, 2);
    drop(other); drop(store); drop(db);
    let db = Db::open(&root.join("db")).unwrap();
    assert_eq!(ResourceStore::new(db.clone(), &scope, "memory").unwrap().get(id).unwrap().unwrap().revision, 2);
    db.put(cf::EXECUTION_STATE, b"nx/resources/schema", b"99").unwrap();
    assert!(ResourceStore::prepare(&db, &root.join("backup")).is_err());
    assert!(ResourceStore::new(db, &scope, "memory").unwrap().write(id, 2, Uuid::new_v4(), json!({})).is_err());
    let backup = Db::open(&root.join("backup")).unwrap();
    assert_eq!(backup.get(cf::CHAT_THREADS, b"legacy").unwrap().unwrap(), b"untouched");
    assert_eq!(ResourceStore::inspect(&backup).unwrap(), None);
}
