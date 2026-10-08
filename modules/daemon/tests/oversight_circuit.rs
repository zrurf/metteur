use metteur_daemon::{
    execution::{DbCheckpointSink, ExecutionEvent, Interpreter},
    llm::{LlmClientFactory, MockClient},
    oversight::scheduler,
    registry::Registry,
    sandbox::approval::{ApprovalBroker, Decision, Scope},
    storage::persistence::Db,
};
use metteur_shared::config::{Config, LlmModelConfig};
use std::{sync::Arc, time::Duration};
use uuid::Uuid;
#[tokio::test]
async fn unavailable_timeout_and_budget_exhausted_circuits_never_dispatch_failed_successors() {
    for (model, extra, delay, status) in [
        (false, serde_json::json!({}), 0, "failed"),
        (true, serde_json::json!({"review_timeout_ms":1}), 100, "timed_out"),
        (true, serde_json::json!({"run_token_budget":0}), 0, "budget_exhausted"),
    ] {
        let root = std::env::temp_dir().join(format!("circuit-failure-{}", Uuid::new_v4()));
        let db = Db::open(&root).unwrap();
        let run = Uuid::new_v4();
        let registry = Arc::new(Registry::with_builtins());
        let bp=metteur_shared::dsl::compile_draft_value_with_catalog(&serde_json::json!({"name":"Circuit","nodes":{"s":{"kind":"Start"},"v":{"kind":"Validator","Actual":1,"Expected":2,"mode":"eq"},"e":{"kind":"End"}},"flow":["s -> v -> e"]}),&registry.authoring_catalog()).unwrap();
        let end = bp.nodes.iter().find(|n| n.kind == "End").unwrap().id;
        let mut config = Config::default();
        config.execution.circuit_break_after = 1;
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
        let client = MockClient::new_delayed(
            vec![metteur_daemon::llm::MockStep::Text(
                r#"{"verdict":"ok","summary":"Late"}"#.into(),
            )],
            Duration::from_millis(delay),
        );
        let broker = Arc::new(ApprovalBroker::new());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut runner =
            Interpreter::new(registry, LlmClientFactory::with_override(Arc::new(client)), root)
                .with_workspace_db(db.clone())
                .with_config(Arc::new(tokio::sync::RwLock::new(config)))
                .with_checkpoint_sink(Arc::new(DbCheckpointSink::new(db.clone(), run)))
                .with_approvals(broker.clone())
                .with_event_tx(tx);
        let respond = tokio::spawn(async move {
            let mut count = 0;
            while let Some(event) = rx.recv().await {
                if let ExecutionEvent::ApprovalRequested {
                    request_id,
                    detail,
                    ..
                } = event
                {
                    assert!(detail.contains("circuit_tripped"));
                    broker
                        .respond(&request_id, Decision::Deny, Scope::Once, &Default::default())
                        .unwrap();
                    count += 1;
                }
            }
            count
        });
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            runner.run(&Arc::new(parking_lot::RwLock::new(bp)), None),
        )
        .await
        .unwrap();
        assert!(result.is_err());
        drop(runner);
        assert_eq!(respond.await.unwrap(), 1);
        let cp = DbCheckpointSink::load(&db, run).unwrap().unwrap();
        assert!(!cp.executed.contains(&end));
        assert!(!cp.pending.contains(&end));
        assert_eq!(cp.circuit_failures, 1);
        let s = scheduler::load(&db, run).unwrap().unwrap();
        assert_eq!(s.reviews.len(), 1);
        assert_eq!(serde_json::to_value(&s.reviews[0].status).unwrap(), status);
        assert!(s.reviews[0].proposals.is_empty());
        assert!(s.reviews[0].circuit_node.is_some());
        assert!(s.reviews[0].verdict.is_none());
    }
}
