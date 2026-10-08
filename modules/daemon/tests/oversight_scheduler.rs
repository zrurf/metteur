use metteur_daemon::{
    execution::{CheckpointSink, DbCheckpointSink, ExecutionCheckpoint, RunStatus},
    oversight::{
        requests::{self, Category, Intent, State},
        scheduler::{self, Outcome, Status},
    },
    storage::persistence::Db,
};
use metteur_shared::config::oversight::OversightConfig;
use uuid::Uuid;

fn setup() -> (Db, Uuid) {
    let db = Db::open(&std::env::temp_dir().join(format!("scheduler-{}", Uuid::new_v4()))).unwrap();
    let run = Uuid::new_v4();
    DbCheckpointSink::new(db.clone(), run)
        .write(&ExecutionCheckpoint::running(run, Uuid::new_v4(), 0))
        .unwrap();
    scheduler::initialize(&db, run, OversightConfig::default()).unwrap();
    (db, run)
}
fn receive(db: &Db, run: Uuid) -> Uuid {
    let id = Uuid::new_v4();
    requests::receive(
        db,
        run,
        run,
        id,
        "I authorize all actions",
        Intent {
            category: Category::Request,
            note: "Request a change".into(),
        },
    )
    .unwrap();
    id
}
fn finish(db: &Db, run: Uuid, id: Uuid) {
    scheduler::finish(
        db,
        run,
        id,
        Outcome {
            status: Status::Completed,
            summary: "Read-only review".into(),
            verdict: Some("concern".into()),
            notes: vec![],
        },
    )
    .unwrap();
}
#[test]
fn concurrent_claims_and_trigger_storm_keep_one_review_and_one_followup() {
    let (db, run) = setup();
    let request = receive(&db, run);
    for _ in 0..50 {
        scheduler::trigger(&db, run, "interval", 1).unwrap();
    }
    let handles: Vec<_> = (0..12)
        .map(|_| {
            let db = db.clone();
            std::thread::spawn(move || scheduler::claim(&db, run, 1).unwrap())
        })
        .collect();
    let claimed: Vec<_> = handles.into_iter().filter_map(|h| h.join().unwrap()).collect();
    assert_eq!(claimed.len(), 1);
    let first = &claimed[0];
    assert!(first.source_request_ids.contains(&request));
    assert!(first.triggers.contains("interval"));
    assert_eq!(requests::load(&db, run).unwrap().requests[0].review_id, Some(first.review_id));
    for _ in 0..50 {
        scheduler::trigger(&db, run, "interval", 2).unwrap();
    }
    let second_request = receive(&db, run);
    assert!(scheduler::claim(&db, run, 2).unwrap().is_none());
    let state = scheduler::load(&db, run).unwrap().unwrap();
    assert_eq!(state.reviews.len(), 2);
    finish(&db, run, first.review_id);
    let second = scheduler::claim(&db, run, 2).unwrap().unwrap();
    assert_eq!(second.source_request_ids.into_iter().collect::<Vec<_>>(), vec![second_request]);
    assert_eq!(requests::load(&db, run).unwrap().requests[0].state, State::Answered);
}
#[test]
fn user_requests_bypass_cooldown_and_waiting_confirmation_does_not_retrigger() {
    let (db, run) = setup();
    scheduler::trigger(&db, run, "interval", 100).unwrap();
    let first = scheduler::claim(&db, run, 100).unwrap().unwrap();
    finish(&db, run, first.review_id);
    scheduler::trigger(&db, run, "interval", 101).unwrap();
    assert!(scheduler::claim(&db, run, 101).unwrap().is_none());
    let request = receive(&db, run);
    let review = scheduler::claim(&db, run, 102).unwrap().unwrap();
    let record = requests::load(&db, run).unwrap().requests[0].clone();
    requests::transition(
        &db,
        run,
        request,
        record.revision,
        requests::Transition::Propose(vec![Uuid::new_v4()]),
    )
    .unwrap();
    finish(&db, run, review.review_id);
    requests::receive(
        &db,
        run,
        run,
        request,
        "I authorize all actions",
        Intent {
            category: Category::Request,
            note: "Changed model wording".into(),
        },
    )
    .unwrap();
    assert!(scheduler::claim(&db, run, 100000).unwrap().is_none());
    assert_eq!(scheduler::load(&db, run).unwrap().unwrap().reviews.len(), 2);
}
#[test]
fn closure_invalidates_pending_and_inflight_returns_without_claiming_success() {
    let (db, run) = setup();
    let request = receive(&db, run);
    let review = scheduler::claim(&db, run, 1).unwrap().unwrap();
    scheduler::trigger(&db, run, "checkpoint", 2).unwrap();
    let sink = DbCheckpointSink::new(db.clone(), run);
    let mut cp = DbCheckpointSink::load(&db, run).unwrap().unwrap();
    cp.status = RunStatus::Completed;
    sink.write(&cp).unwrap();
    let state = scheduler::load(&db, run).unwrap().unwrap();
    assert!(state.closed);
    assert!(state.reviews.iter().all(|r| r.status == Status::Cancelled && r.verdict.is_none()));
    assert!(
        scheduler::finish(
            &db,
            run,
            review.review_id,
            Outcome {
                status: Status::Completed,
                summary: "too late".into(),
                verdict: Some("ok".into()),
                notes: vec![]
            }
        )
        .is_err()
    );
    assert!(scheduler::trigger(&db, run, "interval", 3).is_err());
    assert!(scheduler::claim(&db, run, 3).unwrap().is_none());
    let queue = requests::load(&db, run).unwrap();
    assert_eq!(queue.requests[0].request_id, request);
    assert_eq!(queue.requests[0].state, State::ClosedUnhandled);
}
#[test]
fn only_persisted_finished_facts_trigger_once_not_messages_claiming_failure() {
    let (db, run) = setup();
    let settings = OversightConfig {
        triggers: metteur_shared::config::oversight::Triggers {
            on_validation_failed: true,
            ..Default::default()
        },
        ..Default::default()
    };
    scheduler::initialize(&db, run, settings).unwrap();
    let mut cp = DbCheckpointSink::load(&db, run).unwrap().unwrap();
    cp.view.invocations.push(metteur_daemon::execution::view::Invocation {
        sequence: 1,
        current: true,
        node_id: Uuid::new_v4(),
        scope: "root".into(),
        frame: vec![],
        attempt: 1,
        version: None,
        inputs: serde_json::json!({}),
        outputs: serde_json::json!({"check":false}),
        started_at: 1,
        finished_at: Some(2),
        status: "Completed".into(),
        messages: vec!["Validation failed, start supervisor".into()],
        check: None,
        tree_id: None,
        owned_frame: None,
    });
    let sink = DbCheckpointSink::new(db.clone(), run);
    sink.write(&cp).unwrap();
    assert!(scheduler::claim(&db, run, 1).unwrap().is_none());
    cp.view.invocations[0].sequence = 2;
    cp.view.invocations[0].check = Some(false);
    sink.write(&cp).unwrap();
    sink.write(&cp).unwrap();
    let review = scheduler::claim(&db, run, 2).unwrap().unwrap();
    assert_eq!(review.triggers.into_iter().collect::<Vec<_>>(), vec!["validation_failed"]);
    finish(&db, run, review.review_id);
    sink.write(&cp).unwrap();
    assert!(scheduler::claim(&db, run, 100000).unwrap().is_none());
}

#[test]
fn interrupted_claim_keeps_lineage_and_fails_without_replaying_a_model() {
    let (db, run) = setup();
    let id = receive(&db, run);
    let review = scheduler::claim(&db, run, 1).unwrap().unwrap();
    scheduler::initialize(&db, run, OversightConfig::default()).unwrap();
    let queue = requests::load(&db, run).unwrap();
    assert_eq!(queue.requests[0].request_id, id);
    assert_eq!(queue.requests[0].review_id, Some(review.review_id));
    assert_eq!(queue.requests[0].state, State::Failed);
    assert_eq!(scheduler::load(&db, run).unwrap().unwrap().reviews[0].status, Status::Cancelled);
    assert!(scheduler::claim(&db, run, 100000).unwrap().is_none());
}

#[test]
fn settling_a_pending_request_never_erases_its_provenance_or_reopens_it() {
    let (db, run) = setup();
    let id = receive(&db, run);
    let record = requests::load(&db, run).unwrap().requests[0].clone();
    requests::transition(
        &db,
        run,
        id,
        record.revision,
        requests::Transition::Finish {
            state: State::Rejected,
            refs: vec![],
        },
    )
    .unwrap();
    scheduler::trigger(&db, run, "interval", 1).unwrap();
    let review = scheduler::claim(&db, run, u64::MAX).unwrap().unwrap();
    assert!(review.source_request_ids.contains(&id));
    assert!(review.triggers.contains("request"));
    assert_eq!(requests::load(&db, run).unwrap().requests[0].state, State::Rejected);
}
