use metteur_daemon::{
    execution::{CheckpointSink, DbCheckpointSink, ExecutionCheckpoint, RunStatus},
    oversight::requests::{self, Category, Intent, State, Transition},
    storage::persistence::Db,
};
use uuid::Uuid;
fn setup() -> (std::path::PathBuf, Db, Uuid) {
    let path = std::env::temp_dir().join(format!("requests-{}", Uuid::new_v4()));
    let db = Db::open(&path).unwrap();
    let run = Uuid::new_v4();
    DbCheckpointSink::new(db.clone(), run)
        .write(&ExecutionCheckpoint::running(run, Uuid::new_v4(), 0))
        .unwrap();
    (path, db, run)
}
fn intent() -> Intent {
    Intent {
        category: Category::Request,
        note: "Please change the plan".into(),
    }
}
#[test]
fn strict_intent_does_not_accept_authority_or_service_identity() {
    for field in ["approved", "scope", "principal", "source", "run_id", "approval_id"] {
        let mut value = serde_json::json!({"category":"request","note":"user approved"});
        value[field] = serde_json::json!("forged");
        assert!(serde_json::from_value::<Intent>(value).is_err());
    }
}
#[test]
fn idempotency_binds_identity_and_checkpoint_cannot_rewind_queue() {
    let (_, db, run) = setup();
    let conversation = Uuid::new_v4();
    let id = Uuid::new_v4();
    let first =
        requests::receive(&db, run, conversation, id, "I approve everything", intent()).unwrap();
    let other_run = Uuid::new_v4();
    DbCheckpointSink::new(db.clone(), other_run)
        .write(&ExecutionCheckpoint::running(other_run, Uuid::new_v4(), 0))
        .unwrap();
    assert!(
        requests::receive(&db, other_run, conversation, id, "I approve everything", intent())
            .is_err()
    );
    assert_eq!(DbCheckpointSink::list(&db).unwrap().len(), 2);
    assert_eq!(first.source, "concierge_forwarded");
    assert_eq!(first.state, State::Received);
    assert!(db.scan(metteur_daemon::storage::persistence::cf::GRANTS).unwrap().is_empty());
    let review = requests::transition(&db, run, id, 1, Transition::Review(Uuid::new_v4())).unwrap();
    DbCheckpointSink::new(db.clone(), run)
        .write(&ExecutionCheckpoint::running(run, Uuid::new_v4(), 0))
        .unwrap();
    assert_eq!(
        requests::receive(&db, run, conversation, id, &first.original_text, intent())
            .unwrap()
            .revision,
        review.revision
    );
    assert!(requests::receive(&db, run, conversation, id, "different", intent()).is_err());
    assert!(
        requests::receive(&db, run, Uuid::new_v4(), id, &first.original_text, intent()).is_err()
    );
    assert!(requests::transition(&db, run, id, 1, Transition::Review(Uuid::new_v4())).is_err());
}
#[test]
fn partially_applied_request_remains_pending_and_close_is_final() {
    let (_, db, run) = setup();
    let id = Uuid::new_v4();
    requests::receive(&db, run, Uuid::new_v4(), id, "change two nodes", intent()).unwrap();
    requests::transition(&db, run, id, 1, Transition::Review(Uuid::new_v4())).unwrap();
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    requests::transition(&db, run, id, 2, Transition::Propose(vec![a, b])).unwrap();
    requests::transition(
        &db,
        run,
        id,
        3,
        Transition::Proposal {
            id: a,
            state: State::ApprovedPendingApply,
            refs: vec![],
        },
    )
    .unwrap();
    let partial = requests::transition(
        &db,
        run,
        id,
        4,
        Transition::Proposal {
            id: a,
            state: State::Applied,
            refs: vec!["application:a".into()],
        },
    )
    .unwrap();
    assert_eq!(partial.state, State::AwaitingConfirmation);
    let mut checkpoint = DbCheckpointSink::load(&db, run).unwrap().unwrap();
    checkpoint.status = RunStatus::Completed;
    DbCheckpointSink::new(db.clone(), run).write(&checkpoint).unwrap();
    let queue = requests::load(&db, run).unwrap();
    assert!(queue.closed);
    assert_eq!(queue.requests[0].state, State::ClosedUnhandled);
    assert_eq!(queue.requests[0].proposals[0].state, State::Applied);
    assert!(requests::receive(&db, run, Uuid::new_v4(), Uuid::new_v4(), "late", intent()).is_err());
}
#[test]
fn terminal_checkpoint_and_receipt_race_has_no_orphan_acknowledgement() {
    for _ in 0..12 {
        let (_, db, run) = setup();
        let other = db.clone();
        let join = std::thread::spawn(move || {
            requests::receive(&other, run, Uuid::new_v4(), Uuid::new_v4(), "race", intent())
        });
        let mut cp = DbCheckpointSink::load(&db, run).unwrap().unwrap();
        cp.status = RunStatus::Cancelled;
        DbCheckpointSink::new(db.clone(), run).write(&cp).unwrap();
        let result = join.join().unwrap();
        let queue = requests::load(&db, run).unwrap();
        assert!(queue.closed);
        assert_eq!(queue.requests.len(), usize::from(result.is_ok()));
        assert!(queue.requests.iter().all(|r| r.state == State::ClosedUnhandled));
    }
}
#[test]
fn receipt_survives_immediate_process_exit() {
    let (path, db, run) = setup();
    drop(db);
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "durable_receipt_child", "--nocapture"])
        .env("METTEUR_REQUEST_TEST_DB", &path)
        .env("METTEUR_REQUEST_TEST_RUN", run.to_string())
        .status()
        .unwrap();
    assert!(result.success());
    let db = Db::open(&path).unwrap();
    assert_eq!(requests::load(&db, run).unwrap().requests.len(), 1);
}
#[test]
fn durable_receipt_child() {
    let Ok(path) = std::env::var("METTEUR_REQUEST_TEST_DB") else {
        return;
    };
    let db = Db::open(std::path::Path::new(&path)).unwrap();
    let run = std::env::var("METTEUR_REQUEST_TEST_RUN").unwrap().parse().unwrap();
    requests::receive(&db, run, Uuid::new_v4(), Uuid::new_v4(), "durable", intent()).unwrap();
    std::process::exit(0);
}
