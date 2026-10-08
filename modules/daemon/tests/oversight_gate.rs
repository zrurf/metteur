use metteur_daemon::{
    execution::{DbCheckpointSink, ExecutionEvent, Interpreter, RunStatus},
    llm::{LlmClientFactory, MockClient},
    oversight::scheduler,
    registry::Registry,
    sandbox::approval::{ApprovalBroker, Decision, Scope},
    storage::persistence::Db,
};
use metteur_shared::{
    Value,
    config::{Config, LlmModelConfig},
};
use std::{sync::Arc, time::Duration};
use tokio::sync::RwLock;
use uuid::Uuid;

async fn run_case(
    extra: serde_json::Value,
    response: &str,
    delay: u64,
    asynchronous: bool,
    model: bool,
    disposition: bool,
) -> (metteur_daemon::execution::ExecutionCheckpoint, scheduler::Schedule, usize) {
    let root = std::env::temp_dir().join(format!("gate-{}", Uuid::new_v4()));
    let db = Db::open(&root).unwrap();
    let run = Uuid::new_v4();
    let registry = Arc::new(Registry::with_builtins());
    let bp=metteur_shared::dsl::compile_draft_value_with_catalog(&serde_json::json!({"name":"Gate","nodes":{"s":{"kind":"Start"},"g":{"kind":"OversightCheckpoint","async":asynchronous},"d":{"kind":"Delay","Ms":150},"e":{"kind":"End"}},"flow":["s -> g -> d -> e"]}), &registry.authoring_catalog()).unwrap();
    let versions = Arc::new(metteur_daemon::storage::versioning::VersionManager::new(
        db.clone(),
        root.clone(),
    ));
    metteur_daemon::storage::blueprint_files::save(
        &db,
        &versions,
        &bp,
        "gate.blueprint",
        &serde_json::to_vec(&bp).unwrap(),
        None,
    )
    .unwrap();
    let mut config = Config::default();
    config.extra.insert("oversight".into(), extra);
    if model {
        config.llm.default_model = Some("test".into());
        config.llm.models.insert(
            "test".into(),
            LlmModelConfig {
                api_type: "openai-chat".into(),
                model_id: "test".into(),
                ..Default::default()
            },
        );
    }
    let edit = response == "EDIT";
    let cancel = response == "CANCEL";
    let steps = if edit {
        vec![
            metteur_daemon::llm::MockStep::Tools(vec![metteur_shared::llm::ToolCall {
                id: "edit".into(),
                name: "ProposeBlueprintEdits".into(),
                arguments: serde_json::json!({"summary":"Shorten future delay","edits":[{"op":"set_pin","match":{"kind":"Delay"},"pin":"Ms","value":1}]}),
            }]),
            metteur_daemon::llm::MockStep::Text(
                r#"{"verdict":"concern","summary":"Action still needs validation"}"#.into(),
            ),
        ]
    } else {
        vec![metteur_daemon::llm::MockStep::Text(response.into())]
    };
    let client = MockClient::new_delayed(steps, Duration::from_millis(delay));
    let broker = Arc::new(ApprovalBroker::new());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut runner =
        Interpreter::new(registry, LlmClientFactory::with_override(Arc::new(client)), root)
            .with_workspace_db(db.clone())
            .with_version_manager(versions)
            .with_config(Arc::new(RwLock::new(config)))
            .with_checkpoint_sink(Arc::new(DbCheckpointSink::new(db.clone(), run)))
            .with_approvals(broker.clone())
            .with_event_tx(tx);
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cancel_flag = cancelled.clone();
    let approve = tokio::spawn(async move {
        let mut n = 0;
        while let Some(e) = rx.recv().await {
            if let ExecutionEvent::ApprovalRequested {
                request_id,
                detail,
                ..
            } = e
            {
                assert!(disposition, "unexpected disposition: {detail}");
                assert!(
                    detail.contains("oversight_gate")
                        || (edit && detail.contains("replan_proposal"))
                );
                if cancel {
                    cancel_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                    broker.close();
                    assert!(
                        broker
                            .respond(&request_id, Decision::Allow, Scope::Once, &Default::default())
                            .is_err()
                    );
                    n += 1;
                    continue;
                }
                broker
                    .respond(&request_id, Decision::Allow, Scope::Once, &Default::default())
                    .unwrap();
                n += 1;
            }
        }
        n
    });
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        runner.run_with_control(
            &Arc::new(parking_lot::RwLock::new(bp)),
            None,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            cancelled,
        ),
    )
    .await
    .unwrap();
    if cancel {
        assert!(result.is_err());
    } else {
        result.unwrap();
    }
    drop(runner);
    let n = approve.await.unwrap();
    (
        DbCheckpointSink::load(&db, run).unwrap().unwrap(),
        scheduler::load(&db, run).unwrap().unwrap(),
        n,
    )
}
fn output(cp: &metteur_daemon::execution::ExecutionCheckpoint, name: &str) -> String {
    let node = cp
        .view
        .root
        .as_ref()
        .unwrap()
        .nodes
        .iter()
        .find(|n| n.kind == "OversightCheckpoint")
        .unwrap();
    let pin = node.pins.iter().find(|p| p.name == name).unwrap();
    match &cp.data_values[&pin.id] {
        Value::String(s) => s.clone(),
        other => panic!("wrong output {other:?}"),
    }
}
#[tokio::test]
async fn synchronous_complete_and_concern_preserve_real_conclusions() {
    for verdict in ["ok", "concern"] {
        let (cp, s, n) = run_case(
            serde_json::json!({"mode":"off"}),
            &format!(r#"{{"verdict":"{verdict}","summary":"Review complete"}}"#),
            0,
            false,
            true,
            verdict == "concern",
        )
        .await;
        assert_eq!(cp.status, RunStatus::Completed);
        assert_eq!(output(&cp, "Status"), "completed");
        assert_eq!(output(&cp, "Verdict"), verdict);
        assert_eq!(n, usize::from(verdict == "concern"));
        assert!(s.reviews[0].actual_action_refs.is_empty());
        assert!(cp.view.invocations.iter().all(|i| i.check.is_none()));
    }
}
#[tokio::test]
async fn failed_timeout_budget_and_missing_model_require_fresh_disposition() {
    for (extra, response, delay, model, status) in [
        (serde_json::json!({}), "invalid", 0, true, "failed"),
        (
            serde_json::json!({"review_timeout_ms":1}),
            r#"{"verdict":"ok","summary":"Late"}"#,
            50,
            true,
            "timed_out",
        ),
        (serde_json::json!({"run_token_budget":0}), "unused", 0, true, "budget_exhausted"),
        (serde_json::json!({}), "unused", 0, false, "failed"),
    ] {
        let (cp, s, n) = run_case(extra, response, delay, false, model, true).await;
        assert_eq!(n, 1);
        assert_eq!(output(&cp, "Status"), status);
        assert_eq!(output(&cp, "Verdict"), "");
        assert!(output(&cp, "Notes").contains("User continued"));
        assert!(s.reviews[0].verdict.is_none());
    }
}
#[tokio::test]
async fn async_outputs_never_rewrite_after_report_completion() {
    let (cp, s, n) = run_case(
        serde_json::json!({}),
        r#"{"verdict":"concern","summary":"Later conclusion"}"#,
        50,
        true,
        true,
        false,
    )
    .await;
    assert_eq!(n, 0);
    assert_eq!(output(&cp, "Status"), "pending");
    assert_eq!(output(&cp, "Verdict"), "");
    assert_eq!(output(&cp, "ReviewId"), s.reviews[0].review_id.to_string());
    assert_eq!(s.reviews[0].status, scheduler::Status::Completed);
    assert_eq!(s.reviews[0].verdict.as_deref(), Some("concern"));
}

#[tokio::test]
async fn gate_commits_approved_changes_before_release_without_overwriting_concern() {
    let (cp, s, n) = run_case(serde_json::json!({}), "EDIT", 0, false, true, true).await;
    assert_eq!(n, 2); // One concrete edit and one unresolved-concern disposition.
    assert_eq!(output(&cp, "Verdict"), "concern");
    assert_eq!(
        s.reviews[0].proposals[0].state,
        metteur_daemon::oversight::requests::State::Applied
    );
    assert_eq!(s.reviews[0].verdict.as_deref(), Some("action_taken"));
    assert_eq!(s.reviews[0].model_verdict.as_deref(), Some("concern"));
    assert!(s.reviews[0].actual_action_refs.iter().any(|r| r.starts_with("version:")));
    assert_eq!(s.reviews[0].human_dispositions[0]["action"], "continue_without_validation");
}
#[tokio::test]
async fn cancellation_withdraws_gate_confirmation_and_retains_unfinished_verdict() {
    let (_, s, n) = run_case(serde_json::json!({}), "CANCEL", 0, false, false, true).await;
    assert_eq!(n, 1);
    assert!(s.closed);
    assert!(s.reviews[0].verdict.is_none());
    assert!(s.reviews[0].human_dispositions.is_empty());
}
