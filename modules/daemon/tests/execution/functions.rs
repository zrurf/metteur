//! Interpreter functions tests.

use crate::common::*;

#[tokio::test]
async fn executes_function_call_via_frames() {
    let func = add_function();
    let registry = Arc::new(Registry::with_builtins());
    registry.register_function(func.clone());
    let blueprint = call_function_blueprint("AddFunc");
    let mut interpreter = Interpreter::new(registry, LlmClientFactory::new(), std::env::temp_dir());
    let events = interpreter.run(&shared(blueprint.clone()), None).await.unwrap();

    // Caller result is 5 + 3 = 8 and is reported through a NodeData event.
    let data = events
        .iter()
        .find_map(|e| match e {
            ExecutionEvent::NodeData {
                node_id,
                outputs,
                ..
            } if blueprint.node(*node_id).map(|n| n.kind == "CallFunction").unwrap_or(false) => {
                Some((*node_id, outputs.clone()))
            }
            _ => None,
        })
        .expect("caller produces node data");
    let caller = blueprint.nodes.iter().find(|n| n.kind == "CallFunction").unwrap();
    let result_pin = caller.pins.iter().find(|p| p.name == "Result").unwrap().id;
    let value = data.1.iter().find(|(id, _)| *id == result_pin).map(|(_, v)| v).unwrap();
    assert_eq!(value.as_float(), Some(8.0));

    // The inner Add node reports its origin function for audit grouping.
    let func_id = func.id;
    assert!(events.iter().any(|e| matches!(
        e,
        ExecutionEvent::NodeData { function: Some(id), .. } if *id == func_id
    )));
}

#[tokio::test]
async fn function_recursion_is_depth_limited() {
    // Selfish: FunctionEntry -> CallFunction(Selfish) -> FunctionExit.
    let (id, entry_node, caller, exit_node) =
        (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let (entry_exec, caller_exin, caller_exout, exit_exin) =
        (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let pin = |p_id: Uuid, name: &str, pin_type: PinType| Pin {
        id: p_id,
        name: name.to_string(),
        pin_type,
        data_type: DataType::Void,
        ..Default::default()
    };
    let func = FunctionEntry {
        id,
        name: "Selfish".to_string(),
        description: String::new(),
        signature: metteur_shared::model::function::FunctionSignature::default(),
        body: Blueprint {
            id,
            name: "Selfish".to_string(),
            nodes: vec![
                Node {
                    id: entry_node,
                    node_type: NodeType::Event,
                    kind: "FunctionEntry".to_string(),
                    position: (0.0, 0.0),
                    pins: vec![pin(entry_exec, "Exec", PinType::ExecOutput)],
                    data: serde_json::Value::Null,
                },
                Node {
                    id: caller,
                    node_type: NodeType::Function,
                    kind: "CallFunction".to_string(),
                    position: (0.0, 0.0),
                    pins: vec![
                        pin(caller_exin, "Exec", PinType::ExecInput),
                        pin(caller_exout, "Exec", PinType::ExecOutput),
                    ],
                    data: serde_json::json!({ "function": "Selfish" }),
                },
                Node {
                    id: exit_node,
                    node_type: NodeType::Event,
                    kind: "FunctionExit".to_string(),
                    position: (0.0, 0.0),
                    pins: vec![pin(exit_exin, "Exec", PinType::ExecInput)],
                    data: serde_json::Value::Null,
                },
            ],
            edges: vec![
                Edge {
                    id: Uuid::new_v4(),
                    source_node: entry_node,
                    source_pin: entry_exec,
                    target_node: caller,
                    target_pin: caller_exin,
                },
                Edge {
                    id: Uuid::new_v4(),
                    source_node: caller,
                    source_pin: caller_exout,
                    target_node: exit_node,
                    target_pin: exit_exin,
                },
            ],
            entry_node_id: entry_node,
        },
        source: metteur_shared::model::function::FunctionSource::Builtin,
    };
    let registry = Arc::new(Registry::with_builtins());
    registry.register_function(func.clone());
    let blueprint = call_function_blueprint("Selfish");
    let mut interpreter = Interpreter::new(registry, LlmClientFactory::new(), std::env::temp_dir());
    let result = interpreter.run(&shared(blueprint), None).await;
    assert!(matches!(
        result,
        Err(DaemonError::Execution(msg)) if msg.contains("depth limit exceeded")
    ));
}

#[tokio::test]
async fn resumes_with_function_frame_from_checkpoint() {
    let func = add_function();
    let registry = Arc::new(Registry::with_builtins());
    registry.register_function(func.clone());
    let blueprint = call_function_blueprint("AddFunc");
    let start = blueprint.entry_node_id;
    let caller = blueprint.nodes.iter().find(|n| n.kind == "CallFunction").unwrap().id;
    let start_a = blueprint
        .nodes
        .iter()
        .find(|n| n.id == start)
        .unwrap()
        .pins
        .iter()
        .find(|p| p.name == "A")
        .unwrap()
        .id;
    let start_b = blueprint
        .nodes
        .iter()
        .find(|n| n.id == start)
        .unwrap()
        .pins
        .iter()
        .find(|p| p.name == "B")
        .unwrap()
        .id;
    let entry_node = func.body.entry_node_id;
    let entry_node_obj = func.body.nodes.iter().find(|n| n.id == entry_node).unwrap();
    let entry_a = entry_node_obj.pins.iter().find(|p| p.name == "A").unwrap().id;
    let entry_b = entry_node_obj.pins.iter().find(|p| p.name == "B").unwrap().id;

    let mut frame_sched = Scheduler::default();
    frame_sched.seed(entry_node);
    let checkpoint = ExecutionCheckpoint {
        addon_identity_version: 1,
        addon_packages: Default::default(),
        function_identities: Some(registry.function_identities()),
        view: Default::default(),
        blueprint_version: None,
        transition_version: metteur_daemon::execution::checkpoint::CHECKPOINT_TRANSITION_VERSION,
        in_flight: None,
        run_id: Uuid::new_v4(),
        blueprint_id: blueprint.id,
        status: RunStatus::Running,
        started_at: 0,
        updated_at: 0,
        call_stack: vec![
            Frame {
                node_id: start,
                pc: 0,
                function: None,
            },
            Frame {
                node_id: caller,
                pc: 0,
                function: Some(FunctionBody {
                    id: func.id,
                    scheduler: frame_sched,
                }),
            },
        ],
        data_values: HashMap::from([
            (start_a, Value::Int(5)),
            (start_b, Value::Int(3)),
            (entry_a, Value::Int(5)),
            (entry_b, Value::Int(3)),
        ]),
        executed: vec![start, caller],
        pending: vec![caller],
        triggered: vec![start, caller],
        transaction_log: Vec::new(),
        executed_order: vec![start, caller],
        attempt_counts: HashMap::new(),
        validation_marks: HashMap::new(),
        variables: vec![HashMap::new(), HashMap::new()],
        todos: Vec::new(),
        exec_tree: metteur_daemon::execution::tree::ExecTree::new(),
        frame_trees: Vec::new(),
        foreach_stack: Vec::new(),
        current_tree: None,
        circuit_failures: 0,
        error: None,
    };
    let mut interpreter = Interpreter::new(registry, LlmClientFactory::new(), std::env::temp_dir());
    let events = interpreter
        .resume_with_control(
            &shared(blueprint),
            checkpoint,
            None,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .await
        .unwrap();
    // Function entry/exit (2 each) + inner Add (3) + caller completion (2).
    assert_eq!(events.len(), 9);
}

#[tokio::test]
async fn function_locals_do_not_leak_to_caller() {
    use metteur_shared::model::function::FunctionSignature;

    // Function body: Entry -> VariableSet(tmp) -> Exit.
    let func_id = Uuid::new_v4();
    let (entry_node, set, exit_node) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let pin = |p_id: Uuid, name: &str, pin_type: PinType, data_type: DataType| Pin {
        id: p_id,
        name: name.to_string(),
        pin_type,
        data_type,
        ..Default::default()
    };
    let (en_ex, set_exin, set_exout, set_name, set_val, set_out, ex_exin) = (
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    );
    let func = FunctionEntry {
        id: func_id,
        name: "SetTmp".to_string(),
        description: String::new(),
        signature: FunctionSignature {
            inputs: vec![],
            outputs: vec![],
        },
        body: Blueprint {
            id: func_id,
            name: "SetTmp".to_string(),
            nodes: vec![
                Node {
                    id: entry_node,
                    node_type: NodeType::Event,
                    kind: "FunctionEntry".to_string(),
                    position: (0.0, 0.0),
                    pins: vec![pin(en_ex, "Exec", PinType::ExecOutput, DataType::Void)],
                    data: serde_json::Value::Null,
                },
                Node {
                    id: set,
                    node_type: NodeType::Function,
                    kind: "VariableSet".to_string(),
                    position: (0.0, 0.0),
                    pins: vec![
                        pin(set_exin, "Exec", PinType::ExecInput, DataType::Void),
                        pin(set_exout, "Exec", PinType::ExecOutput, DataType::Void),
                        pin_with_default(
                            set_name,
                            "Name",
                            PinType::DataInput,
                            DataType::String,
                            serde_json::json!("tmp"),
                        ),
                        pin_with_default(
                            set_val,
                            "Value",
                            PinType::DataInput,
                            DataType::Any,
                            serde_json::json!(7),
                        ),
                        pin(set_out, "Value", PinType::DataOutput, DataType::Any),
                    ],
                    data: serde_json::Value::Null,
                },
                Node {
                    id: exit_node,
                    node_type: NodeType::Event,
                    kind: "FunctionExit".to_string(),
                    position: (0.0, 0.0),
                    pins: vec![pin(ex_exin, "Exec", PinType::ExecInput, DataType::Void)],
                    data: serde_json::Value::Null,
                },
            ],
            edges: vec![
                Edge {
                    id: Uuid::new_v4(),
                    source_node: entry_node,
                    source_pin: en_ex,
                    target_node: set,
                    target_pin: set_exin,
                },
                Edge {
                    id: Uuid::new_v4(),
                    source_node: set,
                    source_pin: set_exout,
                    target_node: exit_node,
                    target_pin: ex_exin,
                },
            ],
            entry_node_id: entry_node,
        },
        source: metteur_shared::model::function::FunctionSource::Builtin,
    };
    // Caller: Start -> Call(SetTmp) -> VariableGet(tmp) -> End.
    let (start, caller, get, end) =
        (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let (s_ex, c_exin, c_exout, g_exin, g_name, g_val, e_exin) = (
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    );
    let blueprint = Blueprint {
        id: Uuid::new_v4(),
        name: "leak".to_string(),
        nodes: vec![
            Node {
                id: start,
                node_type: NodeType::Event,
                kind: "Start".to_string(),
                position: (0.0, 0.0),
                pins: vec![pin(s_ex, "Exec", PinType::ExecOutput, DataType::Void)],
                data: serde_json::Value::Null,
            },
            Node {
                id: caller,
                node_type: NodeType::Function,
                kind: "CallFunction".to_string(),
                position: (0.0, 0.0),
                pins: vec![
                    pin(c_exin, "Exec", PinType::ExecInput, DataType::Void),
                    pin(c_exout, "Exec", PinType::ExecOutput, DataType::Void),
                ],
                data: serde_json::json!({ "function": "SetTmp" }),
            },
            Node {
                id: get,
                node_type: NodeType::Pure,
                kind: "VariableGet".to_string(),
                position: (0.0, 0.0),
                pins: vec![
                    pin(g_exin, "Exec", PinType::ExecInput, DataType::Void),
                    pin_with_default(
                        g_name,
                        "Name",
                        PinType::DataInput,
                        DataType::String,
                        serde_json::json!("tmp"),
                    ),
                    pin(g_val, "Value", PinType::DataOutput, DataType::Any),
                ],
                data: serde_json::Value::Null,
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
                target_node: caller,
                target_pin: c_exin,
            },
            Edge {
                id: Uuid::new_v4(),
                source_node: caller,
                source_pin: c_exout,
                target_node: get,
                target_pin: g_exin,
            },
        ],
        entry_node_id: start,
    };
    let registry = Arc::new(Registry::with_builtins());
    registry.register_function(func);
    let mut interpreter = Interpreter::new(registry, LlmClientFactory::new(), std::env::temp_dir());
    let result = interpreter.run(&shared(blueprint), None).await;
    assert!(matches!(
        result,
        Err(DaemonError::Execution(msg)) if msg.contains("undefined variable")
    ));
}
