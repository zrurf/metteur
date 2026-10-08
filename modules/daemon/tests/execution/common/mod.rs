//! Shared imports and fixtures for the interpreter engine tests.

pub use metteur_daemon::Registry;
pub use metteur_daemon::error::{DaemonError, DaemonResult};
pub use metteur_daemon::execution::checkpoint::{CheckpointSink, ExecutionCheckpoint, RunStatus};
pub use metteur_daemon::execution::context::{
    ExecutionContext, Frame, FunctionBody, RetryMark, Scheduler,
};
pub use metteur_daemon::execution::interpreter::{ExecutionEvent, Interpreter};
pub use metteur_daemon::llm::LlmClientFactory;
pub use metteur_shared::model::function::FunctionEntry;
pub use metteur_shared::{Blueprint, DataType, Edge, Node, NodeType, Pin, PinType, Value};
pub use parking_lot::RwLock as PLock;
pub use std::collections::HashMap;
pub use std::path::PathBuf;
pub use std::sync::Arc;
pub use tokio::sync::RwLock;
pub use uuid::Uuid;

pub fn new_interpreter() -> Interpreter {
    Interpreter::new(
        Arc::new(Registry::with_builtins()),
        LlmClientFactory::new(),
        std::env::temp_dir(),
    )
}

/// Wraps a blueprint in the shared handle used by the interpreter.
pub fn shared(bp: Blueprint) -> Arc<PLock<Blueprint>> {
    Arc::new(PLock::new(bp))
}

/// Builds a blueprint: Start -> Add(A=2, B=3) -> Judge(Score=Result).
pub fn build_blueprint() -> Blueprint {
    let start = Uuid::new_v4();
    let add = Uuid::new_v4();
    let judge = Uuid::new_v4();

    let start_exec_out = Uuid::new_v4();
    let start_a = Uuid::new_v4();
    let start_b = Uuid::new_v4();
    let add_exec_in = Uuid::new_v4();
    let add_exec_out = Uuid::new_v4();
    let add_a = Uuid::new_v4();
    let add_b = Uuid::new_v4();
    let add_result = Uuid::new_v4();
    let judge_exec_in = Uuid::new_v4();
    let judge_score = Uuid::new_v4();
    let judge_success = Uuid::new_v4();

    let nodes = vec![
        Node {
            id: start,
            node_type: NodeType::Event,
            kind: "Start".to_string(),
            position: (0.0, 0.0),
            pins: vec![
                Pin {
                    id: start_exec_out,
                    name: "Exec".to_string(),
                    pin_type: PinType::ExecOutput,
                    data_type: DataType::Void,
                    ..Default::default()
                },
                Pin {
                    id: start_a,
                    name: "A".to_string(),
                    pin_type: PinType::DataOutput,
                    data_type: DataType::Float,
                    ..Default::default()
                },
                Pin {
                    id: start_b,
                    name: "B".to_string(),
                    pin_type: PinType::DataOutput,
                    data_type: DataType::Float,
                    ..Default::default()
                },
            ],
            data: serde_json::json!({ "A": 2, "B": 3 }),
        },
        Node {
            id: add,
            node_type: NodeType::Pure,
            kind: "Add".to_string(),
            position: (0.0, 0.0),
            pins: vec![
                Pin {
                    id: add_exec_in,
                    name: "Exec".to_string(),
                    pin_type: PinType::ExecInput,
                    data_type: DataType::Void,
                    ..Default::default()
                },
                Pin {
                    id: add_exec_out,
                    name: "Exec".to_string(),
                    pin_type: PinType::ExecOutput,
                    data_type: DataType::Void,
                    ..Default::default()
                },
                Pin {
                    id: add_a,
                    name: "A".to_string(),
                    pin_type: PinType::DataInput,
                    data_type: DataType::Float,
                    ..Default::default()
                },
                Pin {
                    id: add_b,
                    name: "B".to_string(),
                    pin_type: PinType::DataInput,
                    data_type: DataType::Float,
                    ..Default::default()
                },
                Pin {
                    id: add_result,
                    name: "Result".to_string(),
                    pin_type: PinType::DataOutput,
                    data_type: DataType::Float,
                    ..Default::default()
                },
            ],
            data: serde_json::Value::Null,
        },
        Node {
            id: judge,
            node_type: NodeType::Function,
            kind: "Judge".to_string(),
            position: (0.0, 0.0),
            pins: vec![
                Pin {
                    id: judge_exec_in,
                    name: "Exec".to_string(),
                    pin_type: PinType::ExecInput,
                    data_type: DataType::Void,
                    ..Default::default()
                },
                Pin {
                    id: judge_score,
                    name: "Score".to_string(),
                    pin_type: PinType::DataInput,
                    data_type: DataType::Float,
                    ..Default::default()
                },
                Pin {
                    id: judge_success,
                    name: "Success".to_string(),
                    pin_type: PinType::DataOutput,
                    data_type: DataType::Bool,
                    ..Default::default()
                },
            ],
            data: serde_json::Value::Null,
        },
    ];

    let edges = vec![
        Edge {
            id: Uuid::new_v4(),
            source_node: start,
            source_pin: start_exec_out,
            target_node: add,
            target_pin: add_exec_in,
        },
        Edge {
            id: Uuid::new_v4(),
            source_node: add,
            source_pin: add_exec_out,
            target_node: judge,
            target_pin: judge_exec_in,
        },
        Edge {
            id: Uuid::new_v4(),
            source_node: add,
            source_pin: add_result,
            target_node: judge,
            target_pin: judge_score,
        },
        Edge {
            id: Uuid::new_v4(),
            source_node: start,
            source_pin: start_a,
            target_node: add,
            target_pin: add_a,
        },
        Edge {
            id: Uuid::new_v4(),
            source_node: start,
            source_pin: start_b,
            target_node: add,
            target_pin: add_b,
        },
    ];

    Blueprint {
        id: Uuid::new_v4(),
        name: "test".to_string(),
        nodes,
        edges,
        entry_node_id: start,
    }
}

/// Builds a pure `AddFunc` function: entry(A, B) -> Add -> exit(Result).
pub fn add_function() -> FunctionEntry {
    use metteur_shared::model::function::{FnPin, FunctionEntry, FunctionSignature};
    let (id, entry_node, add, exit_node) =
        (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let (
        entry_exec,
        entry_a,
        entry_b,
        add_exin,
        add_exout,
        add_a,
        add_b,
        add_res,
        exit_exin,
        exit_res,
    ) = (
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
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
    FunctionEntry {
        id,
        name: "AddFunc".to_string(),
        description: "Adds two numbers".to_string(),
        signature: FunctionSignature {
            inputs: vec![
                FnPin {
                    name: "A".to_string(),
                    data_type: DataType::Float,
                    description: None,
                    ..Default::default()
                },
                FnPin {
                    name: "B".to_string(),
                    data_type: DataType::Float,
                    description: None,
                    ..Default::default()
                },
            ],
            outputs: vec![FnPin {
                name: "Result".to_string(),
                data_type: DataType::Float,
                description: None,
                ..Default::default()
            }],
        },
        body: Blueprint {
            id,
            name: "AddFunc".to_string(),
            nodes: vec![
                Node {
                    id: entry_node,
                    node_type: NodeType::Event,
                    kind: "FunctionEntry".to_string(),
                    position: (0.0, 0.0),
                    pins: vec![
                        pin(entry_exec, "Exec", PinType::ExecOutput, DataType::Void),
                        pin(entry_a, "A", PinType::DataOutput, DataType::Float),
                        pin(entry_b, "B", PinType::DataOutput, DataType::Float),
                    ],
                    data: serde_json::Value::Null,
                },
                Node {
                    id: add,
                    node_type: NodeType::Pure,
                    kind: "Add".to_string(),
                    position: (0.0, 0.0),
                    pins: vec![
                        pin(add_exin, "Exec", PinType::ExecInput, DataType::Void),
                        pin(add_exout, "Exec", PinType::ExecOutput, DataType::Void),
                        pin(add_a, "A", PinType::DataInput, DataType::Float),
                        pin(add_b, "B", PinType::DataInput, DataType::Float),
                        pin(add_res, "Result", PinType::DataOutput, DataType::Float),
                    ],
                    data: serde_json::Value::Null,
                },
                Node {
                    id: exit_node,
                    node_type: NodeType::Event,
                    kind: "FunctionExit".to_string(),
                    position: (0.0, 0.0),
                    pins: vec![
                        pin(exit_exin, "Exec", PinType::ExecInput, DataType::Void),
                        pin(exit_res, "Result", PinType::DataInput, DataType::Float),
                    ],
                    data: serde_json::Value::Null,
                },
            ],
            edges: vec![
                Edge {
                    id: Uuid::new_v4(),
                    source_node: entry_node,
                    source_pin: entry_exec,
                    target_node: add,
                    target_pin: add_exin,
                },
                Edge {
                    id: Uuid::new_v4(),
                    source_node: add,
                    source_pin: add_exout,
                    target_node: exit_node,
                    target_pin: exit_exin,
                },
                Edge {
                    id: Uuid::new_v4(),
                    source_node: entry_node,
                    source_pin: entry_a,
                    target_node: add,
                    target_pin: add_a,
                },
                Edge {
                    id: Uuid::new_v4(),
                    source_node: entry_node,
                    source_pin: entry_b,
                    target_node: add,
                    target_pin: add_b,
                },
                Edge {
                    id: Uuid::new_v4(),
                    source_node: add,
                    source_pin: add_res,
                    target_node: exit_node,
                    target_pin: exit_res,
                },
            ],
            entry_node_id: entry_node,
        },
        source: metteur_shared::model::function::FunctionSource::Builtin,
    }
}

/// Builds Start -> CallFunction(AddFunc) with A=5, B=3 directly wired.
pub fn call_function_blueprint(func_name: &str) -> Blueprint {
    let (start, caller) = (Uuid::new_v4(), Uuid::new_v4());
    let (start_ex, start_a, start_b, caller_exin, caller_exout, caller_a, caller_b, caller_res) = (
        Uuid::new_v4(),
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
    Blueprint {
        id: Uuid::new_v4(),
        name: "call-fn".to_string(),
        nodes: vec![
            Node {
                id: start,
                node_type: NodeType::Event,
                kind: "Start".to_string(),
                position: (0.0, 0.0),
                pins: vec![
                    pin(start_ex, "Exec", PinType::ExecOutput, DataType::Void),
                    pin(start_a, "A", PinType::DataOutput, DataType::Float),
                    pin(start_b, "B", PinType::DataOutput, DataType::Float),
                ],
                data: serde_json::json!({ "A": 5, "B": 3 }),
            },
            Node {
                id: caller,
                node_type: NodeType::Function,
                kind: "CallFunction".to_string(),
                position: (0.0, 0.0),
                pins: vec![
                    pin(caller_exin, "Exec", PinType::ExecInput, DataType::Void),
                    pin(caller_exout, "Exec", PinType::ExecOutput, DataType::Void),
                    pin(caller_a, "A", PinType::DataInput, DataType::Float),
                    pin(caller_b, "B", PinType::DataInput, DataType::Float),
                    pin(caller_res, "Result", PinType::DataOutput, DataType::Float),
                ],
                data: serde_json::json!({ "function": func_name }),
            },
        ],
        edges: vec![
            Edge {
                id: Uuid::new_v4(),
                source_node: start,
                source_pin: start_ex,
                target_node: caller,
                target_pin: caller_exin,
            },
            Edge {
                id: Uuid::new_v4(),
                source_node: start,
                source_pin: start_a,
                target_node: caller,
                target_pin: caller_a,
            },
            Edge {
                id: Uuid::new_v4(),
                source_node: start,
                source_pin: start_b,
                target_node: caller,
                target_pin: caller_b,
            },
        ],
        entry_node_id: start,
    }
}

pub fn pin_with_default(
    p_id: Uuid,
    name: &str,
    pin_type: PinType,
    data_type: DataType,
    default: serde_json::Value,
) -> Pin {
    Pin {
        id: p_id,
        name: name.to_string(),
        pin_type,
        data_type,
        default: Some(default),
        ..Default::default()
    }
}

/// An in-memory checkpoint sink for tests.
#[derive(Default)]
pub struct MemorySink {
    run_id: uuid::Uuid,
    pub checkpoints: std::sync::Mutex<Vec<ExecutionCheckpoint>>,
}

impl MemorySink {
    pub fn new() -> Self {
        Self {
            run_id: Uuid::new_v4(),
            checkpoints: std::sync::Mutex::new(Vec::new()),
        }
    }
}

impl CheckpointSink for MemorySink {
    fn run_id(&self) -> uuid::Uuid {
        self.run_id
    }

    fn write(&self, checkpoint: &ExecutionCheckpoint) -> DaemonResult<()> {
        self.checkpoints.lock().unwrap().push(checkpoint.clone());
        Ok(())
    }
}

/// A sink that records every checkpoint a run produces, standing in for a
/// crash that can be resumed from any of them.
pub struct RecordSink {
    run_id: uuid::Uuid,
    pub checkpoints: std::sync::Mutex<Vec<ExecutionCheckpoint>>,
}

impl RecordSink {
    pub fn new() -> Self {
        Self {
            run_id: Uuid::new_v4(),
            checkpoints: std::sync::Mutex::new(Vec::new()),
        }
    }
}

impl CheckpointSink for RecordSink {
    fn run_id(&self) -> uuid::Uuid {
        self.run_id
    }

    fn write(&self, checkpoint: &ExecutionCheckpoint) -> DaemonResult<()> {
        self.checkpoints.lock().unwrap().push(checkpoint.clone());
        Ok(())
    }
}

/// Builds a checkpoint representing the run state right after Start ran.
pub fn checkpoint_after_start(blueprint: &Blueprint) -> ExecutionCheckpoint {
    let start = blueprint.entry_node_id;
    let start_node = blueprint.node(start).unwrap();
    let start_a = start_node.pins.iter().find(|p| p.name == "A").unwrap().id;
    let start_b = start_node.pins.iter().find(|p| p.name == "B").unwrap().id;
    let add = blueprint.nodes.iter().find(|n| n.kind == "Add").unwrap().id;
    ExecutionCheckpoint {
        addon_identity_version: 1,
        addon_packages: Default::default(),
        function_identities: None,
        view: Default::default(),
        blueprint_version: None,
        transition_version: metteur_daemon::execution::checkpoint::CHECKPOINT_TRANSITION_VERSION,
        in_flight: None,
        run_id: Uuid::new_v4(),
        blueprint_id: blueprint.id,
        status: RunStatus::Running,
        started_at: 0,
        updated_at: 0,
        call_stack: Vec::new(),
        data_values: HashMap::from([(start_a, Value::Int(2)), (start_b, Value::Int(3))]),
        executed: vec![start],
        pending: vec![add],
        triggered: vec![start, add],
        transaction_log: Vec::new(),
        executed_order: vec![start],
        attempt_counts: HashMap::new(),
        validation_marks: HashMap::new(),
        variables: vec![HashMap::new()],
        todos: Vec::new(),
        exec_tree: metteur_daemon::execution::tree::ExecTree::new(),
        frame_trees: Vec::new(),
        foreach_stack: Vec::new(),
        current_tree: None,
        circuit_failures: 0,
        error: None,
    }
}
