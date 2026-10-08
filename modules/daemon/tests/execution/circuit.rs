//! Interpreter circuit tests.

use crate::common::*;

fn circuit_config(threshold: u32) -> Arc<RwLock<metteur_shared::config::Config>> {
    use metteur_shared::config::{Config, ExecutionConfig, LlmConfig, LlmModelConfig};
    let mut config = Config {
        execution: ExecutionConfig {
            circuit_break_after: threshold,
            validation_max_attempts: 1,
            ..Default::default()
        },
        llm: LlmConfig {
            default_model: Some("test".into()),
            models: HashMap::from([(
                "test".into(),
                LlmModelConfig {
                    api_type: "openai-chat".into(),
                    model_id: "test".into(),
                    ..Default::default()
                },
            )]),
            ..Default::default()
        },
        ..Default::default()
    };
    config
        .extra
        .insert("oversight".into(), serde_json::json!({"triggers":{"on_validation_failed":true}}));
    Arc::new(RwLock::new(config))
}

#[tokio::test]
async fn approved_replan_saves_only_the_rerun_before_its_successor() {
    use metteur_daemon::llm::MockClient;
    use metteur_daemon::sandbox::approval::{ApprovalBroker, Decision, Scope};
    use std::sync::atomic::AtomicBool;

    let registry = Arc::new(Registry::with_builtins());
    let bp = metteur_shared::dsl::compile_draft_value_with_catalog(&serde_json::json!({
        "name":"circuit", "nodes":{"s":{"kind":"Start"},"v":{"kind":"Validator","Actual":5,"Expected":99,"mode":"eq"},"e":{"kind":"End"}},"flow":["s -> v -> e"]
    }), &registry.authoring_catalog()).unwrap();
    let root = std::env::temp_dir().join(format!("circuit-version-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let db = metteur_daemon::storage::persistence::Db::open(&root.join(".metteur/db")).unwrap();
    let versions = Arc::new(metteur_daemon::storage::versioning::VersionManager::new(
        db.clone(),
        root.clone(),
    ));
    metteur_daemon::storage::blueprint_files::save(
        &db,
        &versions,
        &bp,
        "circuit.blueprint",
        &serde_json::to_vec(&bp).unwrap(),
        None,
    )
    .unwrap();
    let validator = bp.nodes.iter().find(|n| n.kind == "Validator").unwrap().id;
    let end = bp.nodes.iter().find(|n| n.kind == "End").unwrap().id;
    let bp = shared(bp);
    let script =
        r#"[{"op":"set_pin","match":{"kind":"Validator"},"pin":"mode","value":"not_empty"}]"#;
    let broker = Arc::new(ApprovalBroker::new());
    let sink = Arc::new(MemorySink::new());
    struct DurableRecorder(Arc<MemorySink>, metteur_daemon::execution::DbCheckpointSink);
    impl CheckpointSink for DurableRecorder {
        fn run_id(&self) -> Uuid {
            self.0.run_id()
        }
        fn write(&self, cp: &ExecutionCheckpoint) -> DaemonResult<()> {
            self.1.write(cp)?;
            self.0.write(cp)
        }
    }
    let durable = Arc::new(DurableRecorder(
        sink.clone(),
        metteur_daemon::execution::DbCheckpointSink::new(db.clone(), sink.run_id()),
    ));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut runner = Interpreter::new(
        Arc::new(Registry::with_builtins()),
        LlmClientFactory::with_override(Arc::new(MockClient::new(vec![metteur_daemon::llm::MockStep::Tools(vec![metteur_shared::llm::ToolCall{id:"repair".into(),name:"ProposeBlueprintEdits".into(),arguments:serde_json::json!({"summary":"Repair validator mode","edits":serde_json::from_str::<serde_json::Value>(script).unwrap()})}]),metteur_daemon::llm::MockStep::Text(r#"{"verdict":"concern","summary":"Rerun required"}"#.into())]))),
        root.clone(),
    )
    .with_workspace_db(db.clone())
    .with_version_manager(versions.clone())
    .with_config(circuit_config(1))
    .with_approvals(broker.clone())
    .with_checkpoint_sink(durable)
    .with_event_tx(tx);
    let approvals = tokio::spawn(async move {
        let mut count = 0;
        let mut started = Vec::new();
        while let Some(event) = rx.recv().await {
            if let ExecutionEvent::NodeStarted {
                node_id,
            } = &event
            {
                started.push(*node_id);
            }
            if let ExecutionEvent::ApprovalRequested {
                request_id,
                ..
            } = event
            {
                broker
                    .respond(&request_id, Decision::Allow, Scope::Once, &Default::default())
                    .unwrap();
                count += 1;
            }
        }
        (count, started)
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), runner.run(&bp, None))
        .await
        .unwrap()
        .unwrap();
    drop(runner);
    let (approved, started) = approvals.await.unwrap();
    assert_eq!(approved, 1);
    let reports = metteur_daemon::oversight::scheduler::load(&db, sink.run_id()).unwrap().unwrap();
    assert_eq!(
        reports.reviews.len(),
        1,
        "the circuit must own the failure before the generic validation trigger can claim it"
    );
    assert!(reports.reviews[0].triggers.contains("circuit"));
    let validations: Vec<_> =
        started.iter().enumerate().filter(|(_, id)| **id == validator).map(|(i, _)| i).collect();
    assert_eq!(validations.len(), 2);
    assert!(started.iter().position(|id| *id == end).unwrap() > validations[1]);
    let checkpoints = sink.checkpoints.lock().unwrap().clone();
    let rerun = checkpoints
        .iter()
        .find(|cp| {
            cp.pending == vec![validator]
                && cp.in_flight.is_none()
                && !cp.executed.contains(&validator)
                && cp.exec_tree.nodes.values().any(|n| n.label == "Validator")
        })
        .expect("replan must commit a validator rerun, with no stale successor")
        .clone();
    assert_eq!(rerun.circuit_failures, 0);
    let output = Arc::new(MemorySink::new());
    let resumed = Interpreter::new(registry, LlmClientFactory::new(), root)
        .with_workspace_db(db)
        .with_version_manager(versions)
        .with_config(circuit_config(1))
        .with_checkpoint_sink(output.clone())
        .resume_with_control(
            &bp,
            rerun,
            None,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .unwrap();
    let started: Vec<_> = resumed
        .iter()
        .filter_map(|e| match e {
            ExecutionEvent::NodeStarted {
                node_id,
            } => Some(*node_id),
            _ => None,
        })
        .collect();
    assert_eq!(started, vec![validator, end]);
    assert_eq!(output.checkpoints.lock().unwrap().last().unwrap().status, RunStatus::Completed);
}

#[tokio::test]
async fn resumed_circuit_counter_trips_at_the_original_threshold_and_records_failed() {
    use metteur_daemon::sandbox::approval::{ApprovalBroker, Decision, Scope};
    use std::sync::atomic::AtomicBool;

    let mut bp = crate::checkpoint_transitions::failing_validator_blueprint(1);
    let first = bp.nodes.iter().find(|n| n.kind == "Validator").unwrap().clone();
    let end = bp.nodes.iter().find(|n| n.kind == "End").unwrap().id;
    let mut second = first.clone();
    second.id = Uuid::new_v4();
    for pin in &mut second.pins {
        pin.id = Uuid::new_v4();
        if pin.name == "Actual" {
            pin.default = Some(serde_json::json!(0));
        }
    }
    let second_input = second.pins.iter().find(|p| p.pin_type == PinType::ExecInput).unwrap().id;
    let second_output = second.pins.iter().find(|p| p.pin_type == PinType::ExecOutput).unwrap().id;
    let to_end = bp.edges.iter_mut().find(|e| e.target_node == end).unwrap();
    let end_input = to_end.target_pin;
    to_end.target_node = second.id;
    to_end.target_pin = second_input;
    bp.edges.push(Edge {
        id: Uuid::new_v4(),
        source_node: second.id,
        source_pin: second_output,
        target_node: end,
        target_pin: end_input,
    });
    bp.nodes.push(second.clone());
    let bp = shared(bp);
    let sink = Arc::new(MemorySink::new());
    new_interpreter().with_checkpoint_sink(sink.clone()).run(&bp, None).await.unwrap();
    let checkpoint = sink
        .checkpoints
        .lock()
        .unwrap()
        .iter()
        .find(|cp| cp.circuit_failures == 1 && cp.pending.contains(&second.id))
        .unwrap()
        .clone();
    let broker = Arc::new(ApprovalBroker::new());
    let output = Arc::new(MemorySink::new());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut runner = new_interpreter()
        .with_config(circuit_config(2))
        .with_approvals(broker.clone())
        .with_event_tx(tx)
        .with_checkpoint_sink(output.clone());
    let deny = tokio::spawn(async move {
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
                return;
            }
        }
        panic!("restored failures must trip the breaker");
    });
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        runner.resume_with_control(
            &bp,
            checkpoint,
            None,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        ),
    )
    .await
    .unwrap();
    assert!(matches!(result, Err(DaemonError::Execution(msg)) if msg.contains("aborted by user")));
    deny.await.unwrap();
    let records = output.checkpoints.lock().unwrap();
    let terminal = records.last().unwrap();
    assert_eq!(terminal.status, RunStatus::Failed);
    assert_eq!(terminal.circuit_failures, 2);
    assert!(!terminal.executed.contains(&end));
    assert!(!terminal.pending.contains(&end));
}

#[tokio::test]
async fn circuit_breaker_aborts_when_user_denies() {
    use metteur_daemon::sandbox::approval::{ApprovalBroker, Decision, Scope};
    use metteur_shared::config::{Config, ExecutionConfig};

    let broker = Arc::new(ApprovalBroker::new());
    let config = Arc::new(RwLock::new(Config {
        execution: ExecutionConfig {
            circuit_break_after: 1,
            validation_max_attempts: 1,
            foreach_max_iterations: 1000,
            ..Default::default()
        },
        ..Default::default()
    }));

    // Start(A=0, Expected=5) -> Validator(mode eq): A != Expected always.
    let (start, validator) = (Uuid::new_v4(), Uuid::new_v4());
    let (start_ex, start_a, start_exp, v_exin, v_actual, v_expected, v_passed) = (
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    );
    let pin = |p_id: Uuid, name: &str, pin_type: PinType, data_type: DataType| Pin {
        id: p_id,
        name: name.to_string(),
        pin_type,
        data_type,
        ..Default::default()
    };
    let blueprint = Blueprint {
        id: Uuid::new_v4(),
        name: "breaker".to_string(),
        nodes: vec![
            Node {
                id: start,
                node_type: NodeType::Event,
                kind: "Start".to_string(),
                position: (0.0, 0.0),
                pins: vec![
                    pin(start_ex, "Exec", PinType::ExecOutput, DataType::Void),
                    pin(start_a, "A", PinType::DataOutput, DataType::Float),
                    pin(start_exp, "Expected", PinType::DataOutput, DataType::Float),
                ],
                data: serde_json::json!({ "A": 0, "Expected": 5 }),
            },
            Node {
                id: validator,
                node_type: NodeType::Function,
                kind: "Validator".to_string(),
                position: (0.0, 0.0),
                pins: vec![
                    pin(v_exin, "Exec", PinType::ExecInput, DataType::Void),
                    pin(v_actual, "Actual", PinType::DataInput, DataType::Float),
                    pin(v_expected, "Expected", PinType::DataInput, DataType::Float),
                    pin(v_passed, "Passed", PinType::DataOutput, DataType::Bool),
                ],
                data: serde_json::json!({ "mode": "eq" }),
            },
        ],
        edges: vec![
            Edge {
                id: Uuid::new_v4(),
                source_node: start,
                source_pin: start_ex,
                target_node: validator,
                target_pin: v_exin,
            },
            Edge {
                id: Uuid::new_v4(),
                source_node: start,
                source_pin: start_a,
                target_node: validator,
                target_pin: v_actual,
            },
            Edge {
                id: Uuid::new_v4(),
                source_node: start,
                source_pin: start_exp,
                target_node: validator,
                target_pin: v_expected,
            },
        ],
        entry_node_id: start,
    };

    let mut interpreter = Interpreter::new(
        Arc::new(Registry::with_builtins()),
        LlmClientFactory::new(),
        std::env::temp_dir(),
    )
    .with_config(config)
    .with_approvals(broker.clone());
    let blueprint_arc = Arc::new(PLock::new(blueprint));
    let task = tokio::spawn(async move { interpreter.run(&blueprint_arc, None).await });
    // Deny the circuit-tripped approval as soon as it appears.
    loop {
        let ids = broker.pending_ids();
        if !ids.is_empty() {
            broker.respond(&ids[0], Decision::Deny, Scope::Once, &Default::default()).unwrap();
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let result = task.await.unwrap();
    assert!(matches!(
        result,
        Err(DaemonError::Execution(msg)) if msg.contains("aborted by user")
    ));
}
