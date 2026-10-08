use super::*;
use crate::execution::checkpoint::{DbCheckpointSink, RunStatus};
use crate::oversight::{requests, scheduler};
use crate::replan::application::{self, Source};
use crate::sandbox::approval::{ApprovalBroker, Decision, Scope};
use crate::storage::{blueprint_files, persistence::Db, versioning::VersionManager};
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use uuid::Uuid;

async fn confirmed_while_paused(cancel: bool) {
    let root = std::env::temp_dir().join(format!("metteur-paused-apply-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let db = Db::open(&root.join(".metteur/db")).unwrap();
    let versions = Arc::new(VersionManager::new(db.clone(), root.clone()));
    let registry = Arc::new(crate::registry::Registry::with_builtins());
    let graph = metteur_shared::dsl::compile_draft_value_with_catalog(
        &json!({
            "name":"paused edit", "nodes":{"s":{"kind":"Start"},
            "a":{"kind":"Add","A":2,"B":3},"e":{"kind":"End"}},
            "flow":["s -> a -> e"]
        }),
        &registry.authoring_catalog(),
    )
    .unwrap();
    let before = blueprint_files::save(
        &db,
        &versions,
        &graph,
        "paused.blueprint",
        &serde_json::to_vec(&graph).unwrap(),
        None,
    )
    .unwrap();
    let run = Uuid::new_v4();
    let sink = Arc::new(DbCheckpointSink::new(db.clone(), run));
    let broker = Arc::new(ApprovalBroker::new());
    let shared = Arc::new(parking_lot::RwLock::new(graph));
    let mut config = metteur_shared::config::Config::default();
    config.extra.insert("oversight".into(), json!({"mode":"assisted"}));
    let mut interpreter =
        Interpreter::new(registry, crate::llm::LlmClientFactory::new(), root.clone())
            .with_checkpoint_sink(sink.clone())
            .with_workspace_db(db.clone())
            .with_version_manager(versions)
            .with_approvals(broker.clone())
            .with_config(Arc::new(tokio::sync::RwLock::new(config)));
    // Use the real run initialization and loop, but drive a deterministic
    // supervisor action without a provider or background worker.
    interpreter.shared_blueprint = Some(shared.clone());
    interpreter.reset_run(&shared);
    let pause = Arc::new(AtomicBool::new(true));
    let cancelled = Arc::new(AtomicBool::new(false));
    let mut ctx = interpreter.make_context(None, pause.clone(), cancelled.clone()).await;
    application::attach(&ctx, None).unwrap();
    interpreter.write_checkpoint(&ctx).unwrap();
    scheduler::initialize(&db, run, Default::default()).unwrap();
    requests::receive(
        &db,
        run,
        run,
        Uuid::new_v4(),
        "Change only Add.A to 9",
        requests::Intent {
            category: requests::Category::Request,
            note: "A concrete proposal, not approval".into(),
        },
    )
    .unwrap();
    let review = scheduler::claim(&db, run, 1).unwrap().unwrap();
    let mut actions = ctx.child_nested();
    let script = json!([{"op":"set_pin","match":{"kind":"Add","nth":1},"pin":"A","value":9}]);
    let (result, ()) = tokio::join!(
        application::approve(
            &mut actions,
            "Change only Add.A",
            &script,
            Source::Supervisor {
                review_id: review.review_id
            }
        ),
        async {
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                loop {
                    if let Some(id) = broker.pending_ids().first() {
                        assert_eq!(
                            blueprint_files::binding(&db, shared.read().id).unwrap(),
                            Some(before.clone())
                        );
                        broker
                            .respond(id, Decision::Allow, Scope::Once, &Default::default())
                            .unwrap();
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
        }
    );
    result.unwrap();
    assert_eq!(
        requests::load(&db, run).unwrap().requests[0].state,
        requests::State::ApprovedPendingApply
    );
    assert_eq!(blueprint_files::binding(&db, shared.read().id).unwrap(), Some(before.clone()));
    cancelled.store(cancel, Ordering::SeqCst);
    pause.store(false, Ordering::SeqCst);
    let result = interpreter.execute(&shared, &mut ctx).await;
    let cp = DbCheckpointSink::load(&db, run).unwrap().unwrap();
    let state = scheduler::load(&db, run).unwrap().unwrap();
    let proposal = &state.reviews[0].proposals[0];
    if cancel {
        assert!(result.is_err());
        assert_eq!(cp.status, RunStatus::Cancelled);
        assert!(proposal.result_refs.is_empty());
        assert_eq!(blueprint_files::binding(&db, shared.read().id).unwrap(), Some(before));
        assert!(cp.executed.is_empty());
    } else {
        let events = result.unwrap();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, crate::execution::ExecutionEvent::NodeData {outputs,..}
            if outputs.iter().any(|(_, v)| v.as_float() == Some(12.0))))
        );
        assert_eq!(cp.status, RunStatus::Completed);
        assert_ne!(cp.blueprint_version, Some(before));
        assert_eq!(cp.blueprint_version, blueprint_files::binding(&db, shared.read().id).unwrap());
        assert_eq!(proposal.state, requests::State::Applied);
        assert_eq!(proposal.decision_source, "human");
        assert_eq!(proposal.result_refs.len(), 2);
        assert_eq!(
            blueprint_files::decode(&std::fs::read(root.join("paused.blueprint")).unwrap())
                .unwrap(),
            *shared.read()
        );
    }
}

#[tokio::test]
async fn paused_confirmation_applies_before_the_next_node_starts() {
    confirmed_while_paused(false).await;
}

#[tokio::test]
async fn cancellation_precedes_a_confirmed_paused_edit() {
    confirmed_while_paused(true).await;
}
