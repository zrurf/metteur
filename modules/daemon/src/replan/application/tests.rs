use super::*;
use crate::sandbox::approval::{ApprovalBroker, Decision, Scope};
use crate::storage::versioning::VersionManager;
use std::sync::Arc;

fn setup() -> (ExecutionContext, DbCheckpointSink, ExecutionCheckpoint) {
    let root = std::env::temp_dir().join(format!("metteur-apply-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let db = Db::open(&root.join(".metteur/db")).unwrap();
    let versions = Arc::new(VersionManager::new(db.clone(), root.clone()));
    let registry = Arc::new(crate::registry::Registry::with_builtins());
    let graph = metteur_shared::dsl::compile_draft_value_with_catalog(&serde_json::json!({
        "name":"apply", "nodes":{"s":{"kind":"Start"},"llm":{"kind":"CallLLM","prompt":"old"},"e":{"kind":"End"}},"flow":["s -> llm -> e"]
    }), &registry.authoring_catalog()).unwrap();
    let version = blueprint_files::save(
        &db,
        &versions,
        &graph,
        "plan.blueprint",
        &serde_json::to_vec(&graph).unwrap(),
        None,
    )
    .unwrap();
    let mut cp = ExecutionCheckpoint::running(Uuid::new_v4(), graph.id, 1);
    cp.blueprint_version = Some(version);
    cp.pending = vec![graph.entry_node_id];
    let sink = DbCheckpointSink::new(db.clone(), cp.run_id);
    sink.write(&cp).unwrap();
    let mut ctx = ExecutionContext::new(registry, crate::llm::LlmClientFactory::new(), root)
        .with_run(cp.run_id, 1);
    ctx.blueprint = Some(Arc::new(parking_lot::RwLock::new(graph)));
    ctx.version_manager = Some(versions);
    ctx.workspace_db = Some(db);
    ctx.approvals = Some(Arc::new(ApprovalBroker::new()));
    attach(&ctx, None).unwrap();
    (ctx, sink, cp)
}
fn edits() -> serde_json::Value {
    serde_json::json!([{"op":"set_pin","match":{"kind":"CallLLM"},"pin":"prompt","value":"new"}])
}
async fn answer(broker: Arc<ApprovalBroker>, decision: Decision) {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if let Some(id) = broker.pending_ids().first() {
                broker.respond(id, decision, Scope::Once, &Default::default()).unwrap();
                assert!(broker.respond(id, decision, Scope::Once, &Default::default()).is_err());
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}
async fn approved(ctx: &mut ExecutionContext) {
    let broker = ctx.approvals.clone().unwrap();
    let script = edits();
    let (result, ()) = tokio::join!(
        approve(ctx, "change prompt", &script, Source::ModelTool),
        answer(broker, Decision::Allow)
    );
    assert!(result.unwrap().contains("pending"));
}

#[tokio::test]
async fn approval_stages_once_and_commits_file_graph_checkpoint_together() {
    let (mut ctx, sink, mut cp) = setup();
    let before = ctx.blueprint.as_ref().unwrap().read().clone();
    approved(&mut ctx).await;
    assert_eq!(*ctx.blueprint.as_ref().unwrap().read(), before);
    let (events, mut rx) = tokio::sync::mpsc::unbounded_channel();
    ctx.events = Some(events);
    commit_boundary(&ctx, &mut cp, &sink).unwrap();
    let after = ctx.blueprint.as_ref().unwrap().read().clone();
    assert_ne!(before, after);
    assert_eq!(
        blueprint_files::decode(&std::fs::read(ctx.workspace_root.join("plan.blueprint")).unwrap())
            .unwrap(),
        after
    );
    assert_eq!(
        DbCheckpointSink::load(db(&ctx).unwrap(), ctx.run_id).unwrap().unwrap().blueprint_version,
        cp.blueprint_version
    );
    assert_eq!(
        cp.blueprint_version,
        blueprint_files::binding(db(&ctx).unwrap(), after.id).unwrap()
    );
    assert!(
        matches!(rx.try_recv().unwrap(),crate::execution::ExecutionEvent::Message { message,.. } if message.starts_with("replan.applied"))
    );
    let count = ctx.version_manager.as_ref().unwrap().list_snapshots().unwrap().len();
    commit_boundary(&ctx, &mut cp, &sink).unwrap();
    assert!(rx.try_recv().is_err());
    assert_eq!(ctx.version_manager.as_ref().unwrap().list_snapshots().unwrap().len(), count);
    ensure_resolved(db(&ctx).unwrap()).unwrap();
    assert_eq!(DbCheckpointSink::list(db(&ctx).unwrap()).unwrap().len(), 1);
    ctx.attach_file_journal();
    ctx.transaction_log.validate_resume(ctx.run_id).unwrap();
    assert_eq!(ctx.transaction_log.rollback_after(0).unwrap(), 0);
    assert_eq!(
        blueprint_files::decode(&std::fs::read(ctx.workspace_root.join("plan.blueprint")).unwrap())
            .unwrap(),
        after
    );
}

#[tokio::test]
async fn stale_file_graph_and_execution_state_invalidate_waiting_approval() {
    for mode in 0..3 {
        let (mut ctx, sink, mut cp) = setup();
        let graph = ctx.blueprint.clone().unwrap();
        let broker = ctx.approvals.clone().unwrap();
        let path = ctx.workspace_root.join("plan.blueprint");
        let script = edits();
        let (result, ()) =
            tokio::join!(approve(&mut ctx, "change", &script, Source::ModelTool), async {
                tokio::time::timeout(std::time::Duration::from_secs(3), async {
                    while broker.pending_ids().is_empty() {
                        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                    }
                })
                .await
                .unwrap();
                match mode {
                    0 => std::fs::write(path, "external edit").unwrap(),
                    1 => graph.write().name = "concurrent graph".into(),
                    _ => {
                        cp.pending.clear();
                        sink.write(&cp).unwrap();
                    }
                }
                answer(broker, Decision::Allow).await;
            });
        assert!(result.is_err());
        assert!(ctx.blueprint_apply.lock().pending.is_none());
        assert!(
            db(&ctx)
                .unwrap()
                .scan(cf::EXECUTION_STATE)
                .unwrap()
                .iter()
                .all(|(key, _)| !key.starts_with(INTENT_PREFIX))
        );
    }
}

#[tokio::test]
async fn denied_and_completed_node_proposals_have_no_effects() {
    let (mut ctx, sink, mut cp) = setup();
    let original = std::fs::read(ctx.workspace_root.join("plan.blueprint")).unwrap();
    let broker = ctx.approvals.clone().unwrap();
    let script = edits();
    let (result, ()) = tokio::join!(
        approve(&mut ctx, "change", &script, Source::ModelTool),
        answer(broker, Decision::Deny)
    );
    assert!(result.is_err());
    cp.executed.push(
        ctx.blueprint
            .as_ref()
            .unwrap()
            .read()
            .nodes
            .iter()
            .find(|n| n.kind == "CallLLM")
            .unwrap()
            .id,
    );
    sink.write(&cp).unwrap();
    assert!(approve(&mut ctx, "change", &script, Source::ModelTool).await.is_err());
    assert_eq!(std::fs::read(ctx.workspace_root.join("plan.blueprint")).unwrap(), original);
}

struct FailingSink(Uuid);
impl CheckpointSink for FailingSink {
    fn run_id(&self) -> Uuid {
        self.0
    }
    fn write(&self, _: &ExecutionCheckpoint) -> DaemonResult<()> {
        Err(DaemonError::Persistence("injected checkpoint failure".into()))
    }
}

#[tokio::test]
async fn oversight_failure_diagnostics_preserve_real_proposal_causes() {
    use crate::oversight::{diagnostic::{Category, Stage}, scheduler};
    for kind in ["persistence", "storage", "stale", "expiry", "application"] {
        let (mut ctx, sink, mut cp) = setup();
        let proposal_id = if kind == "application" {
            let review = supervisor(&mut ctx, "assisted");
            let broker = ctx.approvals.clone().unwrap();
            let apply = ctx.blueprint_apply.clone();
            let script = edits();
            let (result, ()) = tokio::join!(approve(&mut ctx, "Concurrent application", &script, Source::Supervisor { review_id: review }), async {
                while broker.pending_ids().is_empty() { tokio::time::sleep(std::time::Duration::from_millis(5)).await; }
                apply.lock().blocked = true;
                answer(broker, Decision::Allow).await;
            });
            assert!(result.is_err());
            scheduler::load(db(&ctx).unwrap(), ctx.run_id).unwrap().unwrap().reviews[0].proposals[0].proposal_id
        } else if kind == "expiry" {
            let review = supervisor(&mut ctx, "assisted");
            let id = Uuid::new_v4();
            let mut guard = crate::oversight::actions::register(&ctx, review, id, "PauseRun", serde_json::json!({})).unwrap();
            assert!(guard.confirm_with_timeout(&ctx, 0).await.is_err());
            id
        } else {
            let id = staged_oversight(&mut ctx).await;
            match kind {
                "stale" => {
                    std::fs::write(ctx.workspace_root.join("plan.blueprint"), "external change").unwrap();
                    commit_boundary(&ctx, &mut cp, &sink).unwrap();
                }
                "storage" => {
                    let bytes = ctx.blueprint_apply.lock().pending.as_ref().unwrap().file.clone();
                    let hash = crate::storage::versioning::hash_content(&bytes);
                    db(&ctx).unwrap().put(cf::FILE_BLOBS, hash.as_bytes(), b"collision").unwrap();
                    assert!(commit_boundary(&ctx, &mut cp, &sink).is_err());
                }
                _ => assert!(commit_boundary(&ctx, &mut cp, &FailingSink(ctx.run_id)).is_err()),
            }
            id
        };
        let report = scheduler::load(db(&ctx).unwrap(), ctx.run_id).unwrap().unwrap().reviews.remove(0);
        let proposal = &report.proposals[0];
        assert_eq!(proposal.proposal_id, proposal_id);
        assert_eq!(proposal.state, crate::oversight::requests::State::Failed);
        assert!(proposal.result_refs.is_empty());
        let detail = proposal.diagnostic.as_ref().unwrap();
        let expected = match kind { "persistence" | "storage" => Category::Persistence, "stale" => Category::VersionStale, "expiry" => Category::ApprovalExpired, _ => Category::Application };
        assert_eq!(detail.category, expected, "{kind}: {detail:?}");
        assert_eq!(detail.run_id, ctx.run_id);
        assert_eq!(detail.review_id, report.review_id);
        assert_eq!(detail.proposal_id, Some(proposal_id));
        if kind == "expiry" { assert_eq!(detail.stage, Stage::Approval); }
        let root = ctx.workspace_root.clone();
        let run = ctx.run_id;
        let encoded = serde_json::to_string(&report).unwrap();
        drop(ctx); drop(sink);
        let reopened = Db::open(&root.join(".metteur/db")).unwrap();
        assert_eq!(serde_json::to_string(&scheduler::load(&reopened, run).unwrap().unwrap().reviews[0]).unwrap(), encoded);
    }
}

#[tokio::test]
async fn rejected_review_is_cancelled_with_linked_diagnostic_and_no_model_failure() {
    use crate::oversight::{diagnostic::Category, review, scheduler, requests};
    let (mut ctx, _, _) = setup();
    let review_id = supervisor(&mut ctx, "assisted");
    let database = db(&ctx).unwrap().clone();
    let pending = scheduler::load(&database, ctx.run_id).unwrap().unwrap().reviews.remove(0);
    let config = ctx.config.as_ref().unwrap().read().await.clone();
    let broker = ctx.approvals.clone().unwrap();
    let client = crate::llm::MockClient::new(vec![crate::llm::MockStep::Tools(vec![metteur_shared::llm::ToolCall { id:"control-call".into(), name:"PauseRun".into(), arguments:serde_json::json!({"summary":"Pause for inspection"}) }])]);
    let (result, ()) = tokio::join!(review::evaluate_with_actions(&database, &pending, &config, "test", &client, Some(&mut ctx)), answer(broker, Decision::Deny));
    let result = result.unwrap();
    assert_eq!(result.status, scheduler::Status::Cancelled);
    assert!(result.verdict.is_none());
    assert!(result.actual_action_refs.is_empty());
    let detail = result.diagnostic.unwrap();
    assert_eq!(detail.category, Category::ApprovalRejected);
    assert_eq!(detail.review_id, review_id);
    assert_eq!(detail.proposal_id, Some(result.proposals[0].proposal_id));
    assert_eq!(detail.call_id, result.work.call_ids.first().copied());
    assert_eq!(result.proposals[0].state, requests::State::Rejected);
    assert_eq!(requests::load(&database, ctx.run_id).unwrap().requests[0].state, requests::State::Rejected);
    assert!(!ctx.pause_requested.load(std::sync::atomic::Ordering::SeqCst));
}
#[tokio::test]
async fn checkpoint_failure_leaves_recovery_intent_and_never_reports_applied() {
    let (mut ctx, sink, mut cp) = setup();
    approved(&mut ctx).await;
    let (events, mut rx) = tokio::sync::mpsc::unbounded_channel();
    ctx.events = Some(events);
    assert!(commit_boundary(&ctx, &mut cp, &FailingSink(ctx.run_id)).is_err());
    assert!(ensure_resolved(db(&ctx).unwrap()).is_err());
    assert!(commit_boundary(&ctx, &mut cp, &sink).is_err());
    assert!(rx.try_recv().is_err());
    assert_ne!(
        DbCheckpointSink::load(db(&ctx).unwrap(), ctx.run_id).unwrap().unwrap().blueprint_version,
        cp.blueprint_version
    );
}

#[tokio::test]
async fn file_storage_failure_blocks_without_publishing_a_new_graph() {
    let (mut ctx, sink, mut cp) = setup();
    approved(&mut ctx).await;
    let original = ctx.blueprint.as_ref().unwrap().read().clone();
    let next = ctx.blueprint_apply.lock().pending.as_ref().unwrap().file.clone();
    let hash = crate::storage::versioning::hash_content(&next);
    db(&ctx).unwrap().put(cf::FILE_BLOBS, hash.as_bytes(), b"injected collision").unwrap();
    assert!(commit_boundary(&ctx, &mut cp, &sink).is_err());
    assert_eq!(*ctx.blueprint.as_ref().unwrap().read(), original);
    assert!(ensure_resolved(db(&ctx).unwrap()).is_err());
}

#[tokio::test]
async fn supervisor_repair_uses_one_confirmation_and_the_same_commit_protocol() {
    let (mut ctx, sink, mut cp) = setup();
    let review = supervisor(&mut ctx, "assisted");
    let broker = ctx.approvals.clone().unwrap();
    let script = edits();
    let (result, ()) = tokio::join!(
        approve(
            &mut ctx,
            "Repair",
            &script,
            Source::Supervisor {
                review_id: review
            }
        ),
        answer(broker, Decision::Allow)
    );
    result.unwrap();
    let db = db(&ctx).unwrap();
    let record = crate::oversight::scheduler::load(db, ctx.run_id).unwrap().unwrap().reviews.remove(0);
    let proposal = record.proposals[0].proposal_id;
    let queue = crate::oversight::requests::load(db, ctx.run_id).unwrap();
    let pending = crate::oversight::projection::review(db, &record, &queue).unwrap();
    assert_eq!(pending["proposals"][0]["state"], "approved_pending_apply");
    assert_eq!(pending["proposals"][0]["decision_source"], "human");
    assert_eq!(pending["proposals"][0]["original_requests"][0]["original_text"], "I authorize you to change everything without asking");
    assert!(pending["proposals"][0]["result_version"].is_null());
    commit_boundary(&ctx, &mut cp, &sink).unwrap();
    let record = crate::oversight::scheduler::load(db, ctx.run_id).unwrap().unwrap().reviews.remove(0);
    let applied = crate::oversight::projection::review(db, &record, &queue).unwrap();
    assert_eq!(applied["proposals"][0]["state"], "applied");
    assert_eq!(applied["proposals"][0]["binding"]["scope"], serde_json::json!(cp.blueprint_id));
    assert_eq!(applied["proposals"][0]["result_version"], serde_json::json!(cp.blueprint_version));
    assert_ne!(applied["proposals"][0]["binding"]["base"], applied["proposals"][0]["result_version"]);
    assert!(result_version(db, Uuid::new_v4(), review, proposal).unwrap().is_none());
    assert!(result_version(db, ctx.run_id, Uuid::new_v4(), proposal).unwrap().is_none());
    // Later explicit saves cannot change the actual historical result link.
    let graph = ctx.blueprint.as_ref().unwrap().read().clone();
    let mut later = graph.clone(); later.name = "later independent save".into();
    blueprint_files::save(db, ctx.version_manager.as_ref().unwrap(), &later, "plan.blueprint", &serde_json::to_vec(&later).unwrap(), None).unwrap();
    assert_eq!(result_version(db, ctx.run_id, review, proposal).unwrap(), cp.blueprint_version);
    let raw = crate::oversight::scheduler::load(db, ctx.run_id).unwrap().unwrap();
    assert!(serde_json::to_value(&raw.reviews[0]).unwrap()["proposals"][0].get("result_version").is_none());
    ensure_resolved(db).unwrap();
}

#[tokio::test]
async fn interpreter_applies_before_the_next_node_reads_its_inputs() {
    let (ctx, _, _) = setup();
    let graph = metteur_shared::dsl::compile_draft_value_with_catalog(&serde_json::json!({
        "name":"boundary", "nodes":{
            "s":{"kind":"Start"},
            "r":{"kind":"ReplanBlueprint","summary":"change A","edits":[{"op":"set_pin","match":{"kind":"Add"},"pin":"A","value":9}]},
            "a":{"kind":"Add","A":2,"B":3},"e":{"kind":"End"}},
        "flow":["s -> r -> a -> e"]
    }),&ctx.registry.authoring_catalog()).unwrap();
    let db = db(&ctx).unwrap().clone();
    let versions = ctx.version_manager.clone().unwrap();
    blueprint_files::save(
        &db,
        &versions,
        &graph,
        "boundary.blueprint",
        &serde_json::to_vec(&graph).unwrap(),
        None,
    )
    .unwrap();
    let sink = Arc::new(DbCheckpointSink::new(db.clone(), Uuid::new_v4()));
    let broker = ctx.approvals.clone().unwrap();
    let shared = Arc::new(parking_lot::RwLock::new(graph));
    let mut interpreter = crate::execution::Interpreter::new(
        ctx.registry.clone(),
        ctx.llm_factory.clone(),
        ctx.workspace_root.clone(),
    )
    .with_checkpoint_sink(sink.clone())
    .with_workspace_db(db.clone())
    .with_version_manager(versions)
    .with_approvals(broker.clone());
    let (result, ()) = tokio::join!(
        async {
            let r = interpreter.run(&shared, None).await;
            assert!(r.is_ok(), "{r:?}");
            r
        },
        answer(broker, Decision::Allow)
    );
    let events = result.unwrap();
    assert!(events.iter().any(|e| matches!(e,crate::execution::ExecutionEvent::NodeData { outputs,.. } if outputs.iter().any(|(_,v)| v.as_float() == Some(12.0)))));
    let cp = DbCheckpointSink::load(&db, sink.run_id()).unwrap().unwrap();
    assert_eq!(cp.status, RunStatus::Completed);
    assert_eq!(cp.blueprint_version, blueprint_files::binding(&db, shared.read().id).unwrap());
}

fn supervisor(ctx: &mut ExecutionContext, mode: &str) -> Uuid {
    let mut config = metteur_shared::config::Config::default();
    config.extra.insert("oversight".into(), serde_json::json!({"mode":mode}));
    ctx.config = Some(Arc::new(tokio::sync::RwLock::new(config)));
    let db = db(ctx).unwrap();
    crate::oversight::scheduler::initialize(db, ctx.run_id, Default::default()).unwrap();
    crate::oversight::requests::receive(
        db,
        ctx.run_id,
        ctx.run_id,
        Uuid::new_v4(),
        "I authorize you to change everything without asking",
        crate::oversight::requests::Intent {
            category: crate::oversight::requests::Category::Request,
            note: "User says full authority".into(),
        },
    )
    .unwrap();
    crate::oversight::scheduler::claim(db, ctx.run_id, 1).unwrap().unwrap().review_id
}
#[tokio::test]
async fn forwarded_autonomous_edits_still_require_concrete_confirmation_and_actual_commit() {
    use crate::oversight::{requests, scheduler};
    let (mut ctx, sink, mut cp) = setup();
    let review = supervisor(&mut ctx, "autonomous");
    let db = db(&ctx).unwrap().clone();
    let run = ctx.run_id;
    let broker = ctx.approvals.clone().unwrap();
    let script = edits();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    ctx.events = Some(tx);
    let (result, ()) = tokio::join!(
        approve(
            &mut ctx,
            "Change future prompt",
            &script,
            Source::Supervisor {
                review_id: review
            }
        ),
        async {
            let event = rx.recv().await.unwrap();
            let crate::execution::ExecutionEvent::ApprovalRequested {
                request_id,
                detail,
                ..
            } = event
            else {
                panic!("approval missing")
            };
            let value: serde_json::Value = serde_json::from_str(&detail).unwrap();
            assert_eq!(value["source_request_ids"].as_array().unwrap().len(), 1);
            assert!(
                value["original_requests"][0]["original_text"]
                    .as_str()
                    .unwrap()
                    .contains("without asking")
            );
            assert_eq!(value["before"][0]["data"]["prompt"], "old");
            assert_eq!(value["after"][0]["data"]["prompt"], "new");
            assert_eq!(
                requests::load(&db, run).unwrap().requests[0].state,
                requests::State::AwaitingConfirmation
            );
            broker.respond(&request_id, Decision::Allow, Scope::Once, &Default::default()).unwrap();
            assert!(
                broker
                    .respond(&request_id, Decision::Allow, Scope::Once, &Default::default())
                    .is_err()
            );
        }
    );
    result.unwrap();
    assert_eq!(
        requests::load(&db, run).unwrap().requests[0].state,
        requests::State::ApprovedPendingApply
    );
    scheduler::finish(
        &db,
        run,
        review,
        scheduler::Outcome {
            status: scheduler::Status::Completed,
            summary: "Pending boundary".into(),
            verdict: Some("concern".into()),
            notes: vec![],
        },
    )
    .unwrap();
    assert!(scheduler::load(&db, run).unwrap().unwrap().reviews[0].actual_action_refs.is_empty());
    commit_boundary(&ctx, &mut cp, &sink).unwrap();
    let report = &scheduler::load(&db, run).unwrap().unwrap().reviews[0];
    assert_eq!(report.verdict.as_deref(), Some("action_taken"));
    assert_eq!(report.actual_action_refs.len(), 2);
    assert_eq!(requests::load(&db, run).unwrap().requests[0].state, requests::State::Applied);
    let versions = ctx.version_manager.as_ref().unwrap().list_snapshots().unwrap().len();
    commit_boundary(&ctx, &mut cp, &sink).unwrap();
    assert_eq!(ctx.version_manager.as_ref().unwrap().list_snapshots().unwrap().len(), versions);
}
#[tokio::test]
async fn supervisor_rejected_stale_and_off_proposals_leave_files_unchanged() {
    for mode in ["deny", "stale", "off"] {
        let (mut ctx, sink, mut cp) = setup();
        let review = supervisor(
            &mut ctx,
            if mode == "off" {
                "off"
            } else {
                "assisted"
            },
        );
        let original = std::fs::read(ctx.workspace_root.join("plan.blueprint")).unwrap();
        let broker = ctx.approvals.clone().unwrap();
        let script = edits();
        if mode == "off" {
            assert!(
                approve(
                    &mut ctx,
                    "change",
                    &script,
                    Source::Supervisor {
                        review_id: review
                    }
                )
                .await
                .is_err()
            );
            assert!(broker.pending_ids().is_empty());
            continue;
        }
        let (result, ()) = tokio::join!(
            approve(
                &mut ctx,
                "change",
                &script,
                Source::Supervisor {
                    review_id: review
                }
            ),
            async {
                while broker.pending_ids().is_empty() {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
                if mode == "stale" {
                    cp.pending.clear();
                    sink.write(&cp).unwrap();
                }
                answer(
                    broker,
                    if mode == "deny" {
                        Decision::Deny
                    } else {
                        Decision::Allow
                    },
                )
                .await;
            }
        );
        assert!(result.is_err());
        assert!(ctx.blueprint_apply.lock().pending.is_none());
        assert_eq!(std::fs::read(ctx.workspace_root.join("plan.blueprint")).unwrap(), original);
        let request =
            &crate::oversight::requests::load(db(&ctx).unwrap(), ctx.run_id).unwrap().requests[0];
        assert_eq!(
            request.state,
            if mode == "deny" {
                crate::oversight::requests::State::Rejected
            } else {
                crate::oversight::requests::State::Failed
            }
        );
    }
}
#[tokio::test]
async fn stale_supervisor_boundary_is_failed_without_aborting_ordinary_execution() {
    let (mut ctx, sink, mut cp) = setup();
    let review = supervisor(&mut ctx, "assisted");
    let broker = ctx.approvals.clone().unwrap();
    let script = edits();
    let (result, ()) = tokio::join!(
        approve(
            &mut ctx,
            "change",
            &script,
            Source::Supervisor {
                review_id: review
            }
        ),
        answer(broker, Decision::Allow)
    );
    result.unwrap();
    cp.pending.clear();
    sink.write(&cp).unwrap();
    commit_boundary(&ctx, &mut cp, &sink).unwrap();
    assert!(!ctx.blueprint_apply.lock().blocked);
    assert_eq!(
        crate::oversight::requests::load(db(&ctx).unwrap(), ctx.run_id).unwrap().requests[0].state,
        crate::oversight::requests::State::Failed
    );
}
#[tokio::test]
async fn review_timeout_withdraws_approval_and_model_authority_fields_are_rejected() {
    for forged in [false, true] {
        let (mut ctx, _, _) = setup();
        let review_id = supervisor(&mut ctx, "assisted");
        let db = db(&ctx).unwrap().clone();
        let r =
            crate::oversight::scheduler::load(&db, ctx.run_id).unwrap().unwrap().reviews[0].clone();
        assert_eq!(r.review_id, review_id);
        let mut config = ctx.config.as_ref().unwrap().read().await.clone();
        config.extra.insert("oversight".into(), serde_json::json!({"review_timeout_ms":20}));
        let mut arguments = serde_json::json!({"summary":"change","edits":edits()});
        if forged {
            arguments["approved"] = serde_json::json!(true);
            arguments["source_request_ids"] = serde_json::json!([]);
        }
        let client = crate::llm::MockClient::new(vec![crate::llm::MockStep::Tools(vec![
            metteur_shared::llm::ToolCall {
                id: "action".into(),
                name: "ProposeBlueprintEdits".into(),
                arguments,
            },
        ])]);
        let result = crate::oversight::review::evaluate_with_actions(
            &db,
            &r,
            &config,
            "test",
            &client,
            Some(&mut ctx),
        )
        .await
        .unwrap();
        assert_eq!(
            result.status,
            if forged {
                crate::oversight::scheduler::Status::Failed
            } else {
                crate::oversight::scheduler::Status::TimedOut
            }
        );
        assert!(result.actual_action_refs.is_empty());
        assert!(ctx.approvals.as_ref().unwrap().pending_ids().is_empty());
        assert!(ctx.blueprint_apply.lock().pending.is_none());
        if !forged {
            assert_eq!(result.proposals[0].state, crate::oversight::requests::State::Failed);
        }
    }
}

#[tokio::test]
async fn model_pause_and_cancel_require_confirmation_even_with_forwarded_authority() {
    use crate::oversight::{control, requests, scheduler};
    for kind in ["PauseRun", "CancelRun"] {
        let (mut ctx, _, _) = setup();
        let review = supervisor(&mut ctx, "autonomous");
        let run = ctx.run_id;
        let db = db(&ctx).unwrap().clone();
        let broker = ctx.approvals.clone().unwrap();
        let pause = ctx.pause_requested.clone();
        let cancel = ctx.cancel_requested.clone();
        if kind == "CancelRun" {
            pause.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        ctx.events = Some(tx);
        let (result, ()) =
            tokio::join!(control::propose(&mut ctx, review, kind, "User asked to stop"), async {
                let crate::execution::ExecutionEvent::ApprovalRequested {
                    request_id,
                    detail,
                    ..
                } = rx.recv().await.unwrap()
                else {
                    panic!("approval missing")
                };
                let detail: serde_json::Value = serde_json::from_str(&detail).unwrap();
                assert_eq!(detail["dangerous"], kind == "CancelRun");
                assert_eq!(detail["source_request_ids"].as_array().unwrap().len(), 1);
                assert!(!cancel.load(std::sync::atomic::Ordering::SeqCst));
                assert_eq!(
                    requests::load(&db, run).unwrap().requests[0].state,
                    requests::State::AwaitingConfirmation
                );
                broker
                    .respond(&request_id, Decision::Allow, Scope::Once, &Default::default())
                    .unwrap();
                assert!(
                    broker
                        .respond(&request_id, Decision::Allow, Scope::Once, &Default::default())
                        .is_err()
                );
            });
        result.unwrap();
        assert!(if kind == "CancelRun" {
            ctx.cancel_requested.load(std::sync::atomic::Ordering::SeqCst)
        } else {
            ctx.pause_requested.load(std::sync::atomic::Ordering::SeqCst)
        });
        assert_eq!(requests::load(&db, run).unwrap().requests[0].state, requests::State::Applied);
        let report = &scheduler::load(&db, run).unwrap().unwrap().reviews[0];
        assert_eq!(report.status, scheduler::Status::Completed);
        assert_eq!(report.verdict.as_deref(), Some("action_taken"));
        assert!(report.actual_action_refs.iter().any(|s| s.starts_with("control:")));
        assert!(control::propose(&mut ctx, review, kind, "Repeat").await.is_err());
    }
}
#[tokio::test]
async fn denied_expired_stale_and_broader_scope_control_never_set_flags() {
    use crate::oversight::control;
    for mode in ["deny", "timeout", "stale", "broad"] {
        let (mut ctx, sink, mut cp) = setup();
        let review = supervisor(&mut ctx, "assisted");
        let broker = ctx.approvals.clone().unwrap();
        if mode == "timeout" {
            assert!(
                tokio::time::timeout(
                    std::time::Duration::from_millis(20),
                    control::propose(&mut ctx, review, "CancelRun", "Cancel")
                )
                .await
                .is_err()
            );
            assert!(broker.pending_ids().is_empty());
        } else {
            let (result, ()) =
                tokio::join!(control::propose(&mut ctx, review, "CancelRun", "Cancel"), async {
                    while broker.pending_ids().is_empty() {
                        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                    }
                    if mode == "stale" {
                        cp.pending.clear();
                        sink.write(&cp).unwrap();
                    }
                    let id = broker.pending_ids()[0].clone();
                    broker
                        .respond(
                            &id,
                            if mode == "deny" {
                                Decision::Deny
                            } else {
                                Decision::Allow
                            },
                            if mode == "broad" {
                                Scope::Run
                            } else {
                                Scope::Once
                            },
                            &Default::default(),
                        )
                        .unwrap();
                });
            assert!(result.is_err());
        }
        assert!(!ctx.cancel_requested.load(std::sync::atomic::Ordering::SeqCst));
        assert!(!ctx.pause_requested.load(std::sync::atomic::Ordering::SeqCst));
        assert!(
            crate::oversight::scheduler::load(db(&ctx).unwrap(), ctx.run_id)
                .unwrap()
                .unwrap()
                .reviews[0]
                .actual_action_refs
                .is_empty()
        );
    }
}
#[tokio::test]
async fn direct_cancel_bypasses_pending_supervisor_and_zero_budget() {
    let (mut ctx, _, _) = setup();
    let review = supervisor(&mut ctx, "assisted");
    ctx.config
        .as_ref()
        .unwrap()
        .write()
        .await
        .extra
        .insert("oversight".into(), serde_json::json!({"run_token_budget":0}));
    let broker = ctx.approvals.clone().unwrap();
    let cancel = ctx.cancel_requested.clone();
    let (result, ()) = tokio::join!(
        crate::oversight::control::propose(&mut ctx, review, "PauseRun", "Pause"),
        async {
            while broker.pending_ids().is_empty() {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
            // The same direct-control primitive used by CancelExecution; no review/budget wait.
            cancel.store(true, std::sync::atomic::Ordering::SeqCst);
            broker.close();
        }
    );
    assert!(result.is_err());
    assert!(ctx.cancel_requested.load(std::sync::atomic::Ordering::SeqCst));
    assert!(!ctx.pause_requested.load(std::sync::atomic::Ordering::SeqCst));
}
#[test]
fn cancellation_result_preserves_partial_conflicts_and_retained_blueprint_operations() {
    let (mut ctx, _, _) = setup();
    supervisor(&mut ctx, "assisted");
    let conflict = ctx.workspace_root.join("conflict.txt");
    let restored = ctx.workspace_root.join("restored.txt");
    std::fs::write(&conflict, "external change").unwrap();
    std::fs::write(&restored, "written").unwrap();
    ctx.transaction_log.record_file_write(
        conflict.clone(),
        Some(b"before".to_vec()),
        b"written".to_vec(),
    );
    ctx.transaction_log.record_file_write(
        restored.clone(),
        Some(b"before".to_vec()),
        b"written".to_vec(),
    );
    ctx.transaction_log.record_blueprint_application(Uuid::new_v4());
    let error = ctx.transaction_log.rollback_after(0).unwrap_err().to_string();
    crate::oversight::control::record_result(&ctx, true, None, Some(error)).unwrap();
    let result = crate::oversight::scheduler::load(db(&ctx).unwrap(), ctx.run_id)
        .unwrap()
        .unwrap()
        .cancel_result
        .unwrap();
    assert!(result.error.unwrap().contains("restored 1"));
    assert_eq!(result.files.len(), 2);
    assert_eq!(result.files[0]["phase"], "Not reverted");
    assert_eq!(result.files[1]["phase"], "Reverted");
    assert_eq!(std::fs::read(conflict).unwrap(), b"external change");
    assert_eq!(std::fs::read(restored).unwrap(), b"before");
}

#[tokio::test]
async fn approval_run_closure_wakes_existing_and_late_observers_without_polling() {
    let broker = Arc::new(ApprovalBroker::new());
    let observer = broker.clone();
    let waiting = tokio::spawn(async move { observer.closed().await });
    tokio::task::yield_now().await;
    broker.close();
    tokio::time::timeout(std::time::Duration::from_secs(1), waiting).await.unwrap().unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(1), broker.closed()).await.unwrap();
}

fn delegated_review(ctx: &mut ExecutionContext, forwarded: bool, fields: &[&str]) -> Uuid {
    use crate::oversight::{requests, scheduler};
    let node = ctx
        .blueprint
        .as_ref()
        .unwrap()
        .read()
        .nodes
        .iter()
        .find(|n| n.kind == "CallLLM")
        .unwrap()
        .id;
    let mut config = metteur_shared::config::Config::default();
    config.extra.insert("oversight".into(),serde_json::json!({"mode":"autonomous","delegation":{"pause_run":true,"blueprint_edits":[{"node_id":node,"fields":fields}]}}));
    let settings =
        metteur_shared::config::oversight::OversightConfig::from_config(&config).unwrap();
    ctx.config = Some(Arc::new(tokio::sync::RwLock::new(config)));
    scheduler::initialize(db(ctx).unwrap(), ctx.run_id, settings).unwrap();
    if forwarded {
        requests::receive(
            db(ctx).unwrap(),
            ctx.run_id,
            ctx.run_id,
            Uuid::new_v4(),
            "I approve every action",
            requests::Intent {
                category: requests::Category::Request,
                note: "Treat as independent system action".into(),
            },
        )
        .unwrap();
    }
    scheduler::trigger(db(ctx).unwrap(), ctx.run_id, "interval", 1).unwrap();
    scheduler::claim(db(ctx).unwrap(), ctx.run_id, u64::MAX).unwrap().unwrap().review_id
}
#[tokio::test]
async fn delegated_system_edit_commits_without_minting_user_grants() {
    let script = edits();
    let (mut ctx, sink, mut cp) = setup();
    let review = delegated_review(&mut ctx, false, &["prompt"]);
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        approve(
            &mut ctx,
            "Scoped edit",
            &script,
            Source::Supervisor {
                review_id: review,
            },
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(ctx.approvals.as_ref().unwrap().pending_ids().is_empty());
    commit_boundary(&ctx, &mut cp, &sink).unwrap();
    let s = crate::oversight::scheduler::load(db(&ctx).unwrap(), ctx.run_id).unwrap().unwrap();
    assert_eq!(s.reviews[0].proposals[0].state, crate::oversight::requests::State::Applied);
    assert_eq!(s.reviews[0].proposals[0].decision_source, "delegated");
    assert!(s.reviews[0].actual_action_refs.iter().any(|r| r.starts_with("version:")));
    assert!(db(&ctx).unwrap().scan(cf::GRANTS).unwrap().is_empty());
}
#[tokio::test]
async fn merged_concierge_lineage_still_requires_one_fresh_confirmation() {
    let (mut ctx, sink, mut cp) = setup();
    let review = delegated_review(&mut ctx, true, &["prompt"]);
    let broker = ctx.approvals.clone().unwrap();
    let script = edits();
    let (result, ()) = tokio::join!(
        approve(
            &mut ctx,
            "Scoped edit",
            &script,
            Source::Supervisor {
                review_id: review
            }
        ),
        answer(broker, Decision::Allow)
    );
    result.unwrap();
    commit_boundary(&ctx, &mut cp, &sink).unwrap();
    let s = crate::oversight::scheduler::load(db(&ctx).unwrap(), ctx.run_id).unwrap().unwrap();
    let r = &s.reviews[0];
    assert!(r.triggers.contains("interval"));
    assert!(r.triggers.contains("request"));
    assert_eq!(r.proposals[0].decision_source, "human");
    assert_eq!(r.proposals[0].source_request_ids.len(), 1);
}
#[tokio::test]
async fn out_of_scope_and_ask_mode_cannot_use_delegation() {
    for ask in [false, true] {
        let (mut ctx, _, _) = setup();
        let review = delegated_review(
            &mut ctx,
            false,
            if ask {
                &["prompt"]
            } else {
                &["temperature"]
            },
        );
        if ask {
            ctx.permission_mode = crate::sandbox::PermissionMode::Ask;
        }
        let broker = ctx.approvals.clone().unwrap();
        let before = std::fs::read(ctx.workspace_root.join("plan.blueprint")).unwrap();
        let script = edits();
        let (result, ()) = tokio::join!(
            approve(
                &mut ctx,
                "Change",
                &script,
                Source::Supervisor {
                    review_id: review
                }
            ),
            answer(broker, Decision::Deny)
        );
        assert!(result.is_err());
        assert_eq!(before, std::fs::read(ctx.workspace_root.join("plan.blueprint")).unwrap());
    }
}
#[tokio::test]
async fn revoked_delegation_is_rechecked_at_the_commit_boundary() {
    let (mut ctx, sink, mut cp) = setup();
    let review = delegated_review(&mut ctx, false, &["prompt"]);
    approve(
        &mut ctx,
        "Scoped edit",
        &edits(),
        Source::Supervisor {
            review_id: review,
        },
    )
    .await
    .unwrap();
    let before = std::fs::read(ctx.workspace_root.join("plan.blueprint")).unwrap();
    ctx.config
        .as_ref()
        .unwrap()
        .write()
        .await
        .extra
        .insert("oversight".into(), serde_json::json!({"mode":"autonomous"}));
    commit_boundary(&ctx, &mut cp, &sink).unwrap();
    assert_eq!(before, std::fs::read(ctx.workspace_root.join("plan.blueprint")).unwrap());
    let s = crate::oversight::scheduler::load(db(&ctx).unwrap(), ctx.run_id).unwrap().unwrap();
    assert_eq!(s.reviews[0].proposals[0].state, crate::oversight::requests::State::Failed);
    assert!(s.reviews[0].actual_action_refs.is_empty());
}
#[tokio::test]
async fn delegated_pause_is_reported_while_cancel_always_requires_a_human() {
    let (mut ctx, _, _) = setup();
    let review = delegated_review(&mut ctx, false, &["prompt"]);
    crate::oversight::control::propose(&mut ctx, review, "PauseRun", "Pause for inspection")
        .await
        .unwrap();
    assert!(ctx.pause_requested.load(std::sync::atomic::Ordering::SeqCst));
    let s = crate::oversight::scheduler::load(db(&ctx).unwrap(), ctx.run_id).unwrap().unwrap();
    assert_eq!(s.reviews[0].proposals[0].decision_source, "delegated");
    let (mut ctx, _, _) = setup();
    let review = delegated_review(&mut ctx, false, &["prompt"]);
    let broker = ctx.approvals.clone().unwrap();
    let (result, ()) = tokio::join!(
        crate::oversight::control::propose(&mut ctx, review, "CancelRun", "Stop all work"),
        answer(broker, Decision::Deny)
    );
    assert!(result.is_err());
    assert!(!ctx.cancel_requested.load(std::sync::atomic::Ordering::SeqCst));
}
#[tokio::test]
async fn circuit_reruns_never_inherit_automatic_parameter_delegation() {
    let (mut ctx, _, _) = setup();
    let review = delegated_review(&mut ctx, false, &["prompt"]);
    let mut schedule =
        crate::oversight::scheduler::load(db(&ctx).unwrap(), ctx.run_id).unwrap().unwrap();
    schedule.reviews[0].circuit_node = Some(
        ctx.blueprint
            .as_ref()
            .unwrap()
            .read()
            .nodes
            .iter()
            .find(|n| n.kind == "CallLLM")
            .unwrap()
            .id,
    );
    crate::oversight::scheduler::save(db(&ctx).unwrap(), ctx.run_id, &schedule).unwrap();
    let broker = ctx.approvals.clone().unwrap();
    let script = edits();
    let (result, ()) = tokio::join!(
        approve(
            &mut ctx,
            "Circuit edit",
            &script,
            Source::Supervisor {
                review_id: review
            }
        ),
        answer(broker, Decision::Deny)
    );
    assert!(result.is_err());
    assert!(ctx.blueprint_apply.lock().pending.is_none());
}

async fn staged_oversight(ctx: &mut ExecutionContext) -> Uuid {
    let review = supervisor(ctx, "assisted");
    let broker = ctx.approvals.clone().unwrap();
    let script = edits();
    let (result, ()) = tokio::join!(
        approve(
            ctx,
            "Recoverable edit",
            &script,
            Source::Supervisor {
                review_id: review
            }
        ),
        answer(broker, Decision::Allow)
    );
    result.unwrap();
    ctx.blueprint_apply.lock().pending.as_ref().unwrap().id
}
fn pending_intent(ctx: &ExecutionContext, cp: &ExecutionCheckpoint) -> ApplyIntent {
    ApplyIntent {
        proposal: ctx.blueprint_apply.lock().pending.take().unwrap(),
        checkpoint: cp.clone(),
        after_version: None,
        committed: false,
        abandoned: false,
        recovery_note: String::new(),
    }
}
fn save_intent(ctx: &ExecutionContext, intent: &ApplyIntent) {
    db(ctx)
        .unwrap()
        .put_durable(
            cf::EXECUTION_STATE,
            format!("blueprint-apply:{}", intent.proposal.id).as_bytes(),
            &encode(intent).unwrap(),
        )
        .unwrap();
}
#[tokio::test]
async fn recovery_after_confirmation_closes_old_authority_without_resetting_budget() {
    let (mut ctx, _, cp) = setup();
    let id = staged_oversight(&mut ctx).await;
    let before = std::fs::read(ctx.workspace_root.join("plan.blueprint")).unwrap();
    let settings = metteur_shared::config::oversight::OversightConfig::default();
    crate::oversight::budget::reserve(
        db(&ctx).unwrap(),
        ctx.run_id,
        crate::oversight::budget::Caller::Supervisor,
        "test",
        19,
        &settings,
    )
    .unwrap();
    let budget = serde_json::to_value(
        crate::oversight::budget::load(db(&ctx).unwrap(), ctx.run_id).unwrap(),
    )
    .unwrap();
    ctx.approvals.as_ref().unwrap().close();
    crate::oversight::recovery::recover(db(&ctx).unwrap()).unwrap();
    let schedule =
        crate::oversight::scheduler::load(db(&ctx).unwrap(), ctx.run_id).unwrap().unwrap();
    assert!(schedule.closed);
    assert_eq!(schedule.reviews[0].proposals[0].proposal_id, id);
    assert_eq!(
        schedule.reviews[0].proposals[0].state,
        crate::oversight::requests::State::ClosedUnhandled
    );
    let queue = crate::oversight::requests::load(db(&ctx).unwrap(), ctx.run_id).unwrap();
    assert_eq!(queue.requests[0].state, crate::oversight::requests::State::ClosedUnhandled);
    crate::oversight::recovery::recover(db(&ctx).unwrap()).unwrap();
    assert_eq!(
        serde_json::to_value(queue).unwrap(),
        serde_json::to_value(
            crate::oversight::requests::load(db(&ctx).unwrap(), ctx.run_id).unwrap()
        )
        .unwrap()
    );
    assert_eq!(
        budget,
        serde_json::to_value(
            crate::oversight::budget::load(db(&ctx).unwrap(), ctx.run_id).unwrap()
        )
        .unwrap()
    );
    assert_eq!(before, std::fs::read(ctx.workspace_root.join("plan.blueprint")).unwrap());
    assert_eq!(
        encode(&cp).unwrap(),
        encode(&DbCheckpointSink::load(db(&ctx).unwrap(), ctx.run_id).unwrap().unwrap()).unwrap()
    );
}
#[tokio::test]
async fn recovery_before_version_write_abandons_only_a_proven_unchanged_intent() {
    let (mut ctx, _, cp) = setup();
    staged_oversight(&mut ctx).await;
    let intent = pending_intent(&ctx, &cp);
    save_intent(&ctx, &intent);
    let count = ctx.version_manager.as_ref().unwrap().list_snapshots().unwrap().len();
    assert!(
        reconcile(db(&ctx).unwrap(), ctx.version_manager.as_ref().unwrap()).unwrap().is_empty()
    );
    ensure_resolved(db(&ctx).unwrap()).unwrap();
    assert_eq!(count, ctx.version_manager.as_ref().unwrap().list_snapshots().unwrap().len());
    let bytes = db(&ctx)
        .unwrap()
        .get(cf::EXECUTION_STATE, format!("blueprint-apply:{}", intent.proposal.id).as_bytes())
        .unwrap()
        .unwrap();
    let restored: ApplyIntent = serde_json::from_slice(&bytes).unwrap();
    assert!(restored.abandoned);
    assert!(!restored.committed);
}
#[tokio::test]
async fn recovery_during_version_write_preserves_conflict_and_never_mixes_checkpoint() {
    let (mut ctx, _, cp) = setup();
    staged_oversight(&mut ctx).await;
    let intent = pending_intent(&ctx, &cp);
    save_intent(&ctx, &intent);
    let p = &intent.proposal;
    blueprint_files::save_with_origin(
        db(&ctx).unwrap(),
        ctx.version_manager.as_ref().unwrap(),
        &p.after,
        &p.base.blueprint_uri,
        &p.file,
        Some(&p.base).into(),
        None,
    )
    .unwrap();
    assert_eq!(
        reconcile(db(&ctx).unwrap(), ctx.version_manager.as_ref().unwrap()).unwrap().len(),
        1
    );
    assert!(ensure_resolved(db(&ctx).unwrap()).is_err());
    assert_eq!(
        encode(&cp).unwrap(),
        encode(&DbCheckpointSink::load(db(&ctx).unwrap(), ctx.run_id).unwrap().unwrap()).unwrap()
    );
    let s = crate::oversight::scheduler::load(db(&ctx).unwrap(), ctx.run_id).unwrap().unwrap();
    assert!(s.reviews[0].proposals[0].reason.contains("Manual recovery"));
    assert!(s.reviews[0].actual_action_refs.is_empty());
}
#[tokio::test]
async fn recovery_after_checkpoint_before_acknowledgement_records_once_without_reapplying() {
    let (mut ctx, sink, mut cp) = setup();
    let id = staged_oversight(&mut ctx).await;
    let old_checkpoint = cp.clone();
    let schedule_key = format!("oversight:schedule:{}", ctx.run_id);
    let queue_key = format!("oversight:requests:{}", ctx.run_id);
    let old_schedule =
        db(&ctx).unwrap().get(cf::EXECUTION_STATE, schedule_key.as_bytes()).unwrap().unwrap();
    let old_queue =
        db(&ctx).unwrap().get(cf::EXECUTION_STATE, queue_key.as_bytes()).unwrap().unwrap();
    commit_boundary(&ctx, &mut cp, &sink).unwrap();
    let count = ctx.version_manager.as_ref().unwrap().list_snapshots().unwrap().len();
    let key = format!("blueprint-apply:{id}");
    let mut intent: ApplyIntent = serde_json::from_slice(
        &db(&ctx).unwrap().get(cf::EXECUTION_STATE, key.as_bytes()).unwrap().unwrap(),
    )
    .unwrap();
    intent.committed = false;
    save_intent(&ctx, &intent);
    db(&ctx)
        .unwrap()
        .put_pair_durable(
            cf::EXECUTION_STATE,
            schedule_key.as_bytes(),
            &old_schedule,
            queue_key.as_bytes(),
            &old_queue,
        )
        .unwrap();
    assert!(
        reconcile(db(&ctx).unwrap(), ctx.version_manager.as_ref().unwrap()).unwrap().is_empty()
    );
    crate::oversight::recovery::recover(db(&ctx).unwrap()).unwrap();
    let first =
        db(&ctx).unwrap().get(cf::EXECUTION_STATE, schedule_key.as_bytes()).unwrap().unwrap();
    reconcile(db(&ctx).unwrap(), ctx.version_manager.as_ref().unwrap()).unwrap();
    crate::oversight::recovery::recover(db(&ctx).unwrap()).unwrap();
    assert_eq!(
        first,
        db(&ctx).unwrap().get(cf::EXECUTION_STATE, schedule_key.as_bytes()).unwrap().unwrap()
    );
    let s = crate::oversight::scheduler::load(db(&ctx).unwrap(), ctx.run_id).unwrap().unwrap();
    assert_eq!(s.reviews[0].proposals[0].state, crate::oversight::requests::State::Applied);
    assert_eq!(s.reviews[0].actual_action_refs.len(), 2);
    assert_eq!(
        crate::oversight::requests::load(db(&ctx).unwrap(), ctx.run_id).unwrap().requests[0].state,
        crate::oversight::requests::State::Applied
    );
    assert_eq!(count, ctx.version_manager.as_ref().unwrap().list_snapshots().unwrap().len());
    assert_eq!(
        encode(&cp).unwrap(),
        encode(&DbCheckpointSink::load(db(&ctx).unwrap(), ctx.run_id).unwrap().unwrap()).unwrap()
    );
    ensure_resolved(db(&ctx).unwrap()).unwrap();
    sink.write(&old_checkpoint).unwrap();
    assert_eq!(
        crate::oversight::requests::load(db(&ctx).unwrap(), ctx.run_id).unwrap().requests[0].state,
        crate::oversight::requests::State::Applied
    );
    assert!(
        attach(&ctx, Some(&old_checkpoint)).is_err(),
        "an old checkpoint cannot be mixed with the newly committed file"
    );
}

#[tokio::test]
async fn recovery_while_awaiting_confirmation_cannot_reopen_or_replay_the_proposal() {
    let (mut ctx, _, _) = setup();
    let review = supervisor(&mut ctx, "assisted");
    let id = Uuid::new_v4();
    let mut guard = crate::oversight::actions::register(
        &ctx,
        review,
        id,
        "blueprint_edits",
        serde_json::json!({"summary":"Unconfirmed"}),
    )
    .unwrap();
    guard.staged(); // Simulate a process loss, with no Drop transition or user decision.
    crate::oversight::recovery::recover(db(&ctx).unwrap()).unwrap();
    crate::oversight::scheduler::initialize(db(&ctx).unwrap(), ctx.run_id, Default::default())
        .unwrap();
    let schedule =
        crate::oversight::scheduler::load(db(&ctx).unwrap(), ctx.run_id).unwrap().unwrap();
    assert!(crate::oversight::scheduler::due_at(&schedule).is_none());
    assert_eq!(
        schedule.reviews[0].proposals[0].state,
        crate::oversight::requests::State::ClosedUnhandled
    );
    assert!(crate::oversight::actions::authorize_application(&ctx, id).is_err());
    assert!(schedule.reviews[0].actual_action_refs.is_empty());
}

#[tokio::test]
async fn acknowledged_cancellation_blocks_resume_even_before_the_terminal_checkpoint() {
    let (mut ctx, _, _) = setup();
    let review = supervisor(&mut ctx, "assisted");
    let broker = ctx.approvals.clone().unwrap();
    let (result, ()) = tokio::join!(
        crate::oversight::control::propose(&mut ctx, review, "CancelRun", "Stop"),
        answer(broker, Decision::Allow)
    );
    result.unwrap();
    crate::oversight::recovery::recover(db(&ctx).unwrap()).unwrap();
    assert!(crate::oversight::recovery::ensure_resumable(db(&ctx).unwrap(), ctx.run_id).is_err());
    let schedule =
        crate::oversight::scheduler::load(db(&ctx).unwrap(), ctx.run_id).unwrap().unwrap();
    assert!(schedule.cancel_result.is_none());
    assert!(
        schedule.reviews[0].proposals[0].reason.contains("final rollback outcome is unavailable")
    );
    assert_eq!(
        DbCheckpointSink::load(db(&ctx).unwrap(), ctx.run_id).unwrap().unwrap().status,
        RunStatus::Running
    );
    assert!(crate::oversight::requests::load(db(&ctx).unwrap(), ctx.run_id).unwrap().closed);
    let (ctx, _, _) = setup();
    crate::oversight::recovery::mark_direct_cancel(db(&ctx).unwrap(), ctx.run_id).unwrap();
    let first = crate::oversight::recovery::closing(db(&ctx).unwrap(), ctx.run_id).unwrap();
    crate::oversight::recovery::mark_direct_cancel(db(&ctx).unwrap(), ctx.run_id).unwrap();
    assert_eq!(first, crate::oversight::recovery::closing(db(&ctx).unwrap(), ctx.run_id).unwrap());
    assert!(crate::oversight::recovery::ensure_resumable(db(&ctx).unwrap(), ctx.run_id).is_err());
}
