//! Interpreter retry tests.

use crate::common::*;

/// A test tool that writes bad content on its first call and good content
/// afterwards, exercising validation retry deterministically.
/// With `fail_always` it never recovers, exercising retry exhaustion.
struct FlakyWrite {
    calls: Arc<std::sync::atomic::AtomicUsize>,
    fail_always: bool,
}

#[async_trait::async_trait]
impl metteur_daemon::registry::Tool for FlakyWrite {
    fn name(&self) -> &str {
        "FlakyWrite"
    }
    fn description(&self) -> &str {
        "writes bad content once, then good content"
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"]
        })
    }
    async fn call(&self, args: &[Value], ctx: &mut ExecutionContext) -> DaemonResult<Value> {
        let path = args
            .iter()
            .find_map(|v| match v {
                Value::Json(obj) => obj.get("path").and_then(|p| p.as_str()).map(str::to_string),
                _ => None,
            })
            .ok_or_else(|| DaemonError::Execution("FlakyWrite requires a path".to_string()))?;
        let attempt = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let content = if self.fail_always || attempt == 0 {
            "bad"
        } else {
            "good"
        };
        let resolved = ctx.workspace_root.join(&path);
        let old_content = std::fs::read(&resolved).ok();
        ctx.transaction_log.record_file_write(
            resolved.clone(),
            old_content,
            content.as_bytes().to_vec(),
        );
        std::fs::write(&resolved, content).map_err(DaemonError::Io)?;
        Ok(Value::Bool(true))
    }
}

/// Builds a blueprint: Start -> FlakyWrite -> ReadFile -> Validator -> End.
fn build_retry_blueprint(file: &str, retry_data: serde_json::Value) -> (Blueprint, Uuid) {
    let start = Uuid::new_v4();
    let write = Uuid::new_v4();
    let read = Uuid::new_v4();
    let validator = Uuid::new_v4();
    let end = Uuid::new_v4();
    let pin = |p_id: Uuid, name: &str, pin_type: PinType, data_type: DataType| Pin {
        id: p_id,
        name: name.to_string(),
        pin_type,
        data_type,
        ..Default::default()
    };
    let tool_node = |id: Uuid,
                     tool_name: &str,
                     exec_in: Uuid,
                     exec_out: Uuid,
                     path_pin: Uuid,
                     result_pin: Uuid| Node {
        id,
        node_type: NodeType::Function,
        kind: "Tool".to_string(),
        position: (0.0, 0.0),
        pins: vec![
            pin(exec_in, "Exec", PinType::ExecInput, DataType::Void),
            pin(exec_out, "Exec", PinType::ExecOutput, DataType::Void),
            Pin {
                default: Some(serde_json::json!(file)),
                ..pin(path_pin, "path", PinType::DataInput, DataType::String)
            },
            pin(result_pin, "Result", PinType::DataOutput, DataType::String),
        ],
        data: serde_json::json!({ "tool_name": tool_name }),
    };
    let (s_ex, w_exin, w_exout, w_path, w_res) =
        (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let (r_exin, r_exout, r_path, r_res, r_rawnum) =
        (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let (v_exin, v_exout, v_actual, v_expected, v_passed) =
        (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let e_exin = Uuid::new_v4();
    let blueprint = Blueprint {
        id: Uuid::new_v4(),
        name: "retry".to_string(),
        nodes: vec![
            Node {
                id: start,
                node_type: NodeType::Event,
                kind: "Start".to_string(),
                position: (0.0, 0.0),
                pins: vec![pin(s_ex, "Exec", PinType::ExecOutput, DataType::Void)],
                data: serde_json::Value::Null,
            },
            tool_node(write, "FlakyWrite", w_exin, w_exout, w_path, w_res),
            // Raw content: this blueprint validates the file text itself,
            // so the read must not carry line-number prefixes.
            {
                let mut node = tool_node(read, "ReadFile", r_exin, r_exout, r_path, r_res);
                node.pins.push(Pin {
                    default: Some(serde_json::json!(false)),
                    ..Pin::data("line_numbers", PinType::DataInput, DataType::Bool, r_rawnum)
                });
                node
            },
            Node {
                id: validator,
                node_type: NodeType::Function,
                kind: "Validator".to_string(),
                position: (0.0, 0.0),
                pins: vec![
                    pin(v_exin, "Exec", PinType::ExecInput, DataType::Void),
                    pin(v_exout, "Exec", PinType::ExecOutput, DataType::Void),
                    pin(v_actual, "Actual", PinType::DataInput, DataType::String),
                    Pin {
                        default: Some(serde_json::json!("good")),
                        ..pin(v_expected, "Expected", PinType::DataInput, DataType::String)
                    },
                    pin(v_passed, "Passed", PinType::DataOutput, DataType::Bool),
                ],
                data: retry_data,
            },
            Node {
                id: end,
                node_type: NodeType::Event,
                kind: "End".to_string(),
                position: (0.0, 0.0),
                pins: vec![pin(e_exin, "Exec", PinType::ExecInput, DataType::Void)],
                data: serde_json::Value::Null,
            },
        ],
        edges: vec![
            Edge {
                id: Uuid::new_v4(),
                source_node: start,
                source_pin: s_ex,
                target_node: write,
                target_pin: w_exin,
            },
            Edge {
                id: Uuid::new_v4(),
                source_node: write,
                source_pin: w_exout,
                target_node: read,
                target_pin: r_exin,
            },
            Edge {
                id: Uuid::new_v4(),
                source_node: read,
                source_pin: r_exout,
                target_node: validator,
                target_pin: v_exin,
            },
            Edge {
                id: Uuid::new_v4(),
                source_node: validator,
                source_pin: v_exout,
                target_node: end,
                target_pin: e_exin,
            },
            Edge {
                id: Uuid::new_v4(),
                source_node: read,
                source_pin: r_res,
                target_node: validator,
                target_pin: v_actual,
            },
        ],
        entry_node_id: start,
    };
    (blueprint, validator)
}

fn messages_of(events: &[ExecutionEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            ExecutionEvent::Message {
                message,
                ..
            } => Some(message.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn validator_retry_rolls_back_and_reruns_segment() {
    let workspace = std::env::temp_dir().join(format!("metteur-retry-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&workspace).unwrap();
    let (blueprint, validator) = build_retry_blueprint(
        "target.txt",
        serde_json::json!({ "mode": "eq", "retry": { "max_attempts": 3 } }),
    );

    let registry = Arc::new(Registry::with_builtins());
    registry
        .try_register_tool(Arc::new(FlakyWrite {
            calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            fail_always: false,
        }))
        .unwrap();
    let sink = Arc::new(MemorySink::new());
    let mut interpreter = Interpreter::new(registry, LlmClientFactory::new(), workspace.clone())
        .with_checkpoint_sink(sink.clone());
    let events = interpreter.run(&shared(blueprint), None).await.unwrap();

    // The bad write was rolled back and the segment re-ran with good content.
    assert_eq!(std::fs::read_to_string(workspace.join("target.txt")).unwrap(), "good");
    let messages = messages_of(&events);
    assert!(messages.iter().any(|m| m.contains("rolled back 1 file mutation")));
    assert!(messages.iter().any(|m| m.contains("retrying attempt 2/3")));
    // Passing clears the validator's attempt state.
    assert!(interpreter.attempt_count(validator).is_none());
    assert!(interpreter.validation_mark(validator).is_some());
    // Full segment re-ran: write + read + validator each completed twice.
    let started = events.iter().filter(|e| matches!(e, ExecutionEvent::NodeStarted { .. })).count();
    assert_eq!(started, 9);
    let checkpoints = sink.checkpoints.lock().unwrap();
    let view = &checkpoints.last().unwrap().view;
    let attempts: Vec<_> = view.invocations.iter().filter(|i| i.node_id == validator).collect();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].attempt, 1);
    assert_eq!(attempts[1].attempt, 2);
    assert_ne!(attempts[0].outputs, attempts[1].outputs);
    assert_eq!(attempts[0].check, Some(false));
    assert_eq!(attempts[1].check, Some(true));
    assert!(!attempts[0].current);
    assert!(view.changes.iter().any(|change| matches!(
        change.kind,
        metteur_daemon::execution::blackboard::ChangeKind::Rollback
    )
        && change.invalidates.contains(&attempts[0].sequence)));
    let encoded = serde_json::to_vec(checkpoints.last().unwrap()).unwrap();
    let restored: ExecutionCheckpoint = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(restored.view.invocations.len(), 9);
}

#[tokio::test]
async fn validator_retry_exhaustion_trips_circuit_breaker() {
    use metteur_daemon::sandbox::approval::{ApprovalBroker, Decision, Scope};
    use metteur_shared::config::{Config, ExecutionConfig};

    let workspace = std::env::temp_dir().join(format!("metteur-retry-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&workspace).unwrap();
    let (blueprint, _) = build_retry_blueprint(
        "target.txt",
        serde_json::json!({ "mode": "eq", "retry": { "max_attempts": 2 } }),
    );

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
    let registry = Arc::new(Registry::with_builtins());
    registry
        .try_register_tool(Arc::new(FlakyWrite {
            calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            fail_always: true,
        }))
        .unwrap();
    let mut interpreter = Interpreter::new(registry, LlmClientFactory::new(), workspace)
        .with_config(config)
        .with_approvals(broker.clone());
    let blueprint_arc = Arc::new(PLock::new(blueprint));
    let task = tokio::spawn(async move { interpreter.run(&blueprint_arc, None).await });
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

#[tokio::test]
async fn retry_state_survives_checkpoint_resume() {
    let blueprint = build_blueprint();
    let mut checkpoint = checkpoint_after_start(&blueprint);
    let validator = Uuid::new_v4();
    checkpoint.attempt_counts = HashMap::from([(validator, 1)]);
    checkpoint.validation_marks = HashMap::from([(
        validator,
        RetryMark {
            log_mark: 0,
            order_mark: 1,
        },
    )]);
    checkpoint.executed_order = vec![blueprint.entry_node_id];

    let sink = Arc::new(MemorySink::new());
    let mut interpreter = new_interpreter().with_checkpoint_sink(sink.clone());
    interpreter
        .resume_with_control(
            &shared(blueprint.clone()),
            checkpoint,
            None,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .await
        .unwrap();
    // Retry bookkeeping survives the resume untouched.
    assert_eq!(interpreter.attempt_count(validator), Some(1));
    assert_eq!(
        interpreter.validation_mark(validator),
        Some(RetryMark {
            log_mark: 0,
            order_mark: 1
        })
    );
    // The restored order prefix is intact and the run continued past it.
    let order = interpreter.execution_order();
    assert_eq!(order.first(), Some(&blueprint.entry_node_id));
    assert!(order.len() >= 3);
}
