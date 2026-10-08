//! Interpreter branch tests.

use crate::common::*;

/// Builds Start -> Branch(Condition) -> WriteFile per outcome -> End.
fn branch_blueprint(condition: bool) -> (Blueprint, PathBuf) {
    let workspace = std::env::temp_dir().join(format!("metteur-branch-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&workspace).unwrap();
    let (start, branch, write_t, write_f, end) =
        (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let pin = |p_id: Uuid, name: &str, pin_type: PinType, data_type: DataType| Pin {
        id: p_id,
        name: name.to_string(),
        pin_type,
        data_type,
        ..Default::default()
    };
    let write_node = |id: Uuid, content: &str| {
        let (exin, exout, path, text) =
            (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        Node {
            id,
            node_type: NodeType::Function,
            kind: "Tool".to_string(),
            position: (0.0, 0.0),
            pins: vec![
                pin(exin, "Exec", PinType::ExecInput, DataType::Void),
                pin(exout, "Exec", PinType::ExecOutput, DataType::Void),
                pin_with_default(
                    path,
                    "path",
                    PinType::DataInput,
                    DataType::String,
                    serde_json::json!("hit.txt"),
                ),
                pin_with_default(
                    text,
                    "content",
                    PinType::DataInput,
                    DataType::String,
                    serde_json::json!(content),
                ),
                pin(Uuid::new_v4(), "Result", PinType::DataOutput, DataType::String),
            ],
            data: serde_json::json!({ "tool_name": "WriteFile" }),
        }
    };
    let (s_ex, b_exin, b_cond, b_res, b_true, b_false) = (
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    );
    let e_exin = Uuid::new_v4();
    let write_t_node = write_node(write_t, "true");
    let write_f_node = write_node(write_f, "false");
    let t_exin = write_t_node
        .pins
        .iter()
        .find(|p| p.name == "Exec" && p.pin_type == PinType::ExecInput)
        .unwrap()
        .id;
    let f_exin = write_f_node
        .pins
        .iter()
        .find(|p| p.name == "Exec" && p.pin_type == PinType::ExecInput)
        .unwrap()
        .id;
    let t_exout = write_t_node
        .pins
        .iter()
        .find(|p| p.name == "Exec" && p.pin_type == PinType::ExecOutput)
        .unwrap()
        .id;
    let f_exout = write_f_node
        .pins
        .iter()
        .find(|p| p.name == "Exec" && p.pin_type == PinType::ExecOutput)
        .unwrap()
        .id;
    let blueprint = Blueprint {
        id: Uuid::new_v4(),
        name: "branch".to_string(),
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
                id: branch,
                node_type: NodeType::Control,
                kind: "Branch".to_string(),
                position: (0.0, 0.0),
                pins: vec![
                    pin(b_exin, "Exec", PinType::ExecInput, DataType::Void),
                    pin_with_default(
                        b_cond,
                        "Condition",
                        PinType::DataInput,
                        DataType::Bool,
                        serde_json::json!(condition),
                    ),
                    pin(b_res, "Result", PinType::DataOutput, DataType::Bool),
                    pin(b_true, "True", PinType::ExecOutput, DataType::Void),
                    pin(b_false, "False", PinType::ExecOutput, DataType::Void),
                ],
                data: serde_json::Value::Null,
            },
            write_t_node,
            write_f_node,
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
                target_node: branch,
                target_pin: b_exin,
            },
            Edge {
                id: Uuid::new_v4(),
                source_node: branch,
                source_pin: b_true,
                target_node: write_t,
                target_pin: t_exin,
            },
            Edge {
                id: Uuid::new_v4(),
                source_node: branch,
                source_pin: b_false,
                target_node: write_f,
                target_pin: f_exin,
            },
            Edge {
                id: Uuid::new_v4(),
                source_node: write_t,
                source_pin: t_exout,
                target_node: end,
                target_pin: e_exin,
            },
            Edge {
                id: Uuid::new_v4(),
                source_node: write_f,
                source_pin: f_exout,
                target_node: end,
                target_pin: e_exin,
            },
        ],
        entry_node_id: start,
    };
    (blueprint, workspace)
}

#[tokio::test]
async fn branch_routes_true_and_false() {
    for (condition, expected) in [(true, "true"), (false, "false")] {
        let (blueprint, workspace) = branch_blueprint(condition);
        let sink = Arc::new(RecordSink::new());
        let taken_edges: Vec<_> = blueprint
            .edges
            .iter()
            .filter(|e| {
                blueprint.pin(e.source_pin).unwrap().name
                    == if condition {
                        "True"
                    } else {
                        "False"
                    }
            })
            .map(|e| e.id)
            .collect();
        let untaken_edges: Vec<_> = blueprint
            .edges
            .iter()
            .filter(|e| {
                blueprint.pin(e.source_pin).unwrap().name
                    == if condition {
                        "False"
                    } else {
                        "True"
                    }
            })
            .map(|e| e.id)
            .collect();
        let mut interpreter = Interpreter::new(
            Arc::new(Registry::with_builtins()),
            LlmClientFactory::new(),
            workspace.clone(),
        )
        .with_checkpoint_sink(sink.clone());
        interpreter.run(&shared(blueprint), None).await.unwrap();
        let checkpoints = sink.checkpoints.lock().unwrap();
        let view = &checkpoints.last().unwrap().view;
        assert!(taken_edges.iter().all(|id| view.edges.iter().any(|edge| edge.edge_id == *id)));
        assert!(untaken_edges.iter().all(|id| view.edges.iter().all(|edge| edge.edge_id != *id)));
        assert_eq!(view.invocations.len(), 4);
        assert!(view.invocations.iter().all(|i| i.status == "Completed" && i.attempt == 1));
        assert_eq!(std::fs::read_to_string(workspace.join("hit.txt")).unwrap(), expected);
    }
}
