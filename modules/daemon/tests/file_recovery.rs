//! Recovery boundaries exercise real durable records and foreign file content.
use metteur_daemon::error::{DaemonError, DaemonResult};
use metteur_daemon::execution::TransactionLog;
use metteur_daemon::execution::file_journal::*;
use metteur_daemon::storage::persistence::{Db, cf};
use std::{path::PathBuf, sync::Arc};
use uuid::Uuid;

fn fixture() -> (PathBuf, Db, FileJournal) {
    let root = std::env::temp_dir().join(format!("metteur-recovery-{}", Uuid::new_v4()));
    let db = Db::open(&root.join(".metteur/db")).unwrap();
    let journal = FileJournal::new(root.clone(), Arc::new(DbFileJournal(db.clone())));
    (root, db, journal)
}
fn origin() -> FileOrigin {
    FileOrigin {
        run_id: Uuid::new_v4(),
        node_id: Uuid::new_v4(),
        attempt: 1,
        wal_position: 0,
    }
}

#[test]
fn reopen_reconciles_both_sides_of_the_apply_boundary_without_replay() {
    for written in [false, true] {
        let (root, db, journal) = fixture();
        let path = root.join("file");
        std::fs::write(&path, b"before").unwrap();
        journal.prepare(&path, Some(b"after"), origin(), None).unwrap();
        if written {
            std::fs::write(&path, b"after").unwrap();
        }
        drop(journal);
        drop(db);
        let reopened = Db::open(&root.join(".metteur/db")).unwrap();
        let journal = FileJournal::new(root, Arc::new(DbFileJournal(reopened)));
        assert_eq!(journal.reconcile().unwrap().ensure_safe().unwrap().reconciled, 1);
        let phase = journal.store.operations().unwrap()[0].phase;
        assert_eq!(
            phase,
            if written {
                FilePhase::Applied
            } else {
                FilePhase::Reverted
            }
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            if written {
                b"after".as_slice()
            } else {
                b"before".as_slice()
            }
        );
        assert_eq!(journal.reconcile().unwrap().reconciled, 0);
    }
}

#[test]
fn repeated_writes_delete_recreate_rollback_once_in_actual_reverse_order() {
    for existed in [false, true] {
        let (root, _, journal) = fixture();
        let path = root.join("file");
        if existed {
            std::fs::write(&path, b"original").unwrap();
        }
        let log = TransactionLog::new().with_journal(journal.clone());
        let origin = origin();
        for after in
            [Some(b"one".as_slice()), Some(b"two".as_slice()), None, Some(b"three".as_slice())]
        {
            log.mutate_file(&root, &path, after, origin.clone(), None).unwrap();
        }
        assert_eq!(log.rollback_after(0).unwrap(), 4);
        assert_eq!(std::fs::read(&path).ok(), existed.then(|| b"original".to_vec()));
        assert!(
            journal.store.operations().unwrap().iter().all(|op| op.phase == FilePhase::Reverted)
        );
        std::fs::write(&path, b"user after rollback").unwrap();
        assert_eq!(log.rollback_after(0).unwrap(), 0);
        assert_eq!(journal.reconcile().unwrap().reconciled, 0);
        assert_eq!(std::fs::read(&path).unwrap(), b"user after rollback");
    }
}

#[test]
fn partial_rollback_preserves_foreign_content_and_restores_independent_files() {
    let (root, _, journal) = fixture();
    let path = root.join("edited");
    let independent = root.join("independent");
    let log = TransactionLog::new().with_journal(journal.clone());
    let origin = origin();
    log.mutate_file(&root, &independent, Some(b"temporary"), origin.clone(), None).unwrap();
    for bytes in [b"one".as_slice(), b"two".as_slice()] {
        log.mutate_file(&root, &path, Some(bytes), origin.clone(), None).unwrap();
    }
    std::fs::write(&path, b"user change").unwrap();
    let error = log.rollback_after(0).unwrap_err().to_string();
    assert!(error.contains("restored 1"), "{error}");
    assert!(error.contains("edited"));
    assert_eq!(std::fs::read(&path).unwrap(), b"user change");
    assert!(!independent.exists());
    assert!(log.rollback_after(0).is_err());
    assert!(journal.reconcile().unwrap().ensure_safe().is_err());
    assert_eq!(std::fs::read(path).unwrap(), b"user change");
}

#[derive(Debug)]
struct FailPhase {
    inner: DbFileJournal,
    phase: FilePhase,
}
impl FileJournalStore for FailPhase {
    fn save(&self, op: &FileOperation) -> DaemonResult<()> {
        if op.phase == self.phase {
            Err(DaemonError::Persistence("injected undo boundary".into()))
        } else {
            self.inner.save(op)
        }
    }
    fn operations(&self) -> DaemonResult<Vec<FileOperation>> {
        self.inner.operations()
    }
    fn put_blob(&self, bytes: &[u8]) -> DaemonResult<String> {
        self.inner.put_blob(bytes)
    }
    fn blob(&self, hash: &str) -> DaemonResult<Vec<u8>> {
        self.inner.blob(hash)
    }
}

#[test]
fn undo_fence_failure_and_result_failure_are_recoverable_without_double_undo() {
    for failed_phase in [FilePhase::Reverting, FilePhase::Reverted] {
        let (root, db, journal) = fixture();
        let path = root.join("file");
        std::fs::write(&path, b"before").unwrap();
        let mut op = journal.prepare(&path, Some(b"after"), origin(), None).unwrap();
        journal.apply(&mut op).unwrap();
        let failing = FileJournal::new(
            root,
            Arc::new(FailPhase {
                inner: DbFileJournal(db),
                phase: failed_phase,
            }),
        );
        assert!(matches!(failing.rollback_operation(&mut op), Err(DaemonError::Persistence(_))));
        if failed_phase == FilePhase::Reverting {
            assert_eq!(std::fs::read(&path).unwrap(), b"after");
            assert_eq!(journal.store.operations().unwrap()[0].phase, FilePhase::Applied);
        } else {
            assert_eq!(std::fs::read(&path).unwrap(), b"before");
            assert_eq!(journal.store.operations().unwrap()[0].phase, FilePhase::Reverting);
            journal.reconcile().unwrap().ensure_safe().unwrap();
            let mut op = journal.store.operations().unwrap().remove(0);
            assert!(!journal.rollback_operation(&mut op).unwrap());
        }
    }
}

#[test]
fn reconcile_finishes_a_durable_undo_fence_before_the_file_changed() {
    let (root, _, journal) = fixture();
    let path = root.join("file");
    let mut op = journal.prepare(&path, Some(b"created"), origin(), None).unwrap();
    journal.apply(&mut op).unwrap();
    op.phase = FilePhase::Reverting;
    journal.save(&op).unwrap();
    journal.reconcile().unwrap().ensure_safe().unwrap();
    assert!(!path.exists());
    assert_eq!(journal.store.operations().unwrap()[0].phase, FilePhase::Reverted);
}

#[test]
fn prepared_conflict_stays_untouched_until_operator_restores_a_known_image() {
    let (root, _, journal) = fixture();
    let path = root.join("file");
    journal.prepare(&path, Some(b"after"), origin(), None).unwrap();
    std::fs::write(&path, b"foreign").unwrap();
    for _ in 0..2 {
        assert_eq!(journal.reconcile().unwrap().conflicts, vec![path.clone()]);
        assert_eq!(std::fs::read(&path).unwrap(), b"foreign");
    }
    std::fs::write(&path, b"after").unwrap();
    journal.reconcile().unwrap().ensure_safe().unwrap();
    let mut op = journal.store.operations().unwrap().remove(0);
    assert_eq!(op.phase, FilePhase::Applied);
    journal.rollback_operation(&mut op).unwrap();
    assert!(!path.exists());
}

#[test]
fn missing_before_blob_stops_undo_before_any_file_write() {
    let (root, db, journal) = fixture();
    let path = root.join("file");
    std::fs::write(&path, b"before").unwrap();
    let mut op = journal.prepare(&path, Some(b"after"), origin(), None).unwrap();
    journal.apply(&mut op).unwrap();
    db.delete(cf::FILE_BLOBS, op.before.as_ref().unwrap().as_bytes()).unwrap();
    assert!(matches!(journal.rollback_operation(&mut op), Err(DaemonError::Persistence(_))));
    assert_eq!(std::fs::read(path).unwrap(), b"after");
}

#[test]
fn stale_checkpoint_cannot_resume_after_undo_or_unrecorded_file_effect() {
    let (root, _, journal) = fixture();
    let origin = origin();
    let log = TransactionLog::new().with_journal(journal.clone());
    log.mutate_file(&root, &root.join("file"), Some(b"after"), origin.clone(), None).unwrap();
    log.validate_resume(origin.run_id).unwrap();
    let checkpoint = log.entries();
    log.rollback().unwrap();
    assert!(
        TransactionLog::from_entries(checkpoint)
            .with_journal(journal.clone())
            .validate_resume(origin.run_id)
            .is_err()
    );
    log.validate_resume(origin.run_id).unwrap();
    let checkpoint = log.entries();
    log.mutate_file(&root, &root.join("later"), Some(b"new"), origin.clone(), None).unwrap();
    assert!(
        TransactionLog::from_entries(checkpoint)
            .with_journal(journal)
            .validate_resume(origin.run_id)
            .is_err()
    );
}

#[tokio::test]
async fn workspace_open_reports_conflicts_and_allows_manual_resolution() {
    let (root, db, journal) = fixture();
    let path = root.join("file");
    journal.prepare(&path, Some(b"after"), origin(), None).unwrap();
    std::fs::write(&path, b"foreign").unwrap();
    drop(journal);
    drop(db);
    let manager = metteur_daemon::workspace::WorkspaceManager::new();
    let workspace = manager.open(&root).await.unwrap();
    let error = workspace.reconcile_files().unwrap_err().to_string();
    assert!(error.contains("file recovery conflicts"), "{error}");
    assert_eq!(std::fs::read(&path).unwrap(), b"foreign");
    std::fs::remove_file(&path).unwrap();
    workspace.reconcile_files().unwrap();
    assert_eq!(
        FileJournal::new(root, Arc::new(DbFileJournal(workspace.db.clone())))
            .store
            .operations()
            .unwrap()[0]
            .phase,
        FilePhase::Reverted
    );
}

#[cfg(unix)]
#[test]
fn undo_refuses_a_replaced_symlink() {
    let (root, _, journal) = fixture();
    let path = root.join("file");
    let foreign = root.join("foreign");
    let mut op = journal.prepare(&path, Some(b"after"), origin(), None).unwrap();
    journal.apply(&mut op).unwrap();
    std::fs::remove_file(&path).unwrap();
    std::fs::write(&foreign, b"keep").unwrap();
    std::os::unix::fs::symlink(&foreign, &path).unwrap();
    assert!(journal.rollback_operation(&mut op).is_err());
    assert_eq!(std::fs::read(foreign).unwrap(), b"keep");
}
