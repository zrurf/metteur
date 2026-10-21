//! Bounded production-model scale and repeated reopen probe, not a long soak.
use metteur_daemon::{
    execution::{
        ExecutionEvent,
        checkpoint::{CheckpointSink, DbCheckpointSink, ExecutionCheckpoint, RunStatus},
    },
    storage::persistence::Db,
};
use metteur_shared::{Blueprint, Edge, Node, NodeType, Pin, PinType, validate};
use std::{collections::HashMap, time::Instant};
use uuid::Uuid;
#[test]
fn thousand_nodes_ten_thousand_invocations_and_twelve_reopens() {
    let root = std::env::temp_dir().join(format!("nx-scale-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for i in 0..1000 {
        let node = Node {
            id: Uuid::new_v4(),
            kind: if i == 0 {
                "Start"
            } else {
                "Delay"
            }
            .into(),
            node_type: if i == 0 {
                NodeType::Event
            } else {
                NodeType::Function
            },
            position: ((i % 40) as f32 * 250., (i / 40) as f32 * 180.),
            pins: vec![
                Pin::exec(PinType::ExecInput, Uuid::new_v4()),
                Pin::exec(PinType::ExecOutput, Uuid::new_v4()),
            ],
            data: serde_json::json!({"ms":0}),
        };
        if let Some(previous) = nodes.last() {
            let previous: &Node = previous;
            edges.push(Edge {
                id: Uuid::new_v4(),
                source_node: previous.id,
                source_pin: previous.pins[1].id,
                target_node: node.id,
                target_pin: node.pins[0].id,
            });
        }
        nodes.push(node);
    }
    let graph = Blueprint {
        id: Uuid::new_v4(),
        name: "Bounded 1k fixture".into(),
        entry_node_id: nodes[0].id,
        nodes,
        edges,
    };
    let start = Instant::now();
    assert!(validate(&graph).is_ok());
    let validate_ms = start.elapsed().as_millis();
    let mut cp = ExecutionCheckpoint::running(Uuid::new_v4(), graph.id, 1);
    cp.view.root = Some(graph.clone());
    let start = Instant::now();
    for i in 0..10000 {
        let n = graph.nodes[i % 1000].id;
        cp.view.begin(
            &graph,
            n,
            vec![format!("pass-{}", i / 1000)],
            None,
            &HashMap::new(),
            i as u64,
        );
        cp.view.observe(
            &ExecutionEvent::Message {
                node_id: n,
                message: format!("row-{i}: {}", "bounded log ".repeat(20)),
            },
            &graph.id.to_string(),
            i as u64,
        );
        cp.view.observe(
            &ExecutionEvent::NodeFinished {
                node_id: n,
            },
            &graph.id.to_string(),
            i as u64 + 1,
        );
    }
    let construct_ms = start.elapsed().as_millis();
    cp.status = RunStatus::Completed;
    let raw = serde_json::to_vec(&cp).unwrap();
    let db = Db::open(&root.join("db")).unwrap();
    let start = Instant::now();
    DbCheckpointSink::new(db.clone(), cp.run_id).write(&cp).unwrap();
    let write_ms = start.elapsed().as_millis();
    drop(db);
    let start = Instant::now();
    for _ in 0..12 {
        let db = Db::open(&root.join("db")).unwrap();
        let loaded = DbCheckpointSink::load(&db, cp.run_id).unwrap().unwrap();
        assert_eq!(loaded.view.invocations.len(), 10000);
        assert_eq!(loaded.view.root.unwrap().nodes.len(), 1000);
        assert_eq!(loaded.status, RunStatus::Completed);
        assert!(loaded.in_flight.is_none());
    }
    let result = serde_json::json!({"nodes":1000,"invocations":10000,"reopens":12,"validate_ms":validate_ms,"construct_ms":construct_ms,"checkpoint_bytes":raw.len(),"write_ms":write_ms,"reopen_total_ms":start.elapsed().as_millis(),"scope":"structural validation, actual execution view production methods, durable checkpoint and repeated reopen; not 10000 real model calls or wall-clock soak"});
    eprintln!("NX_SCALE {result}");
    std::fs::write(root.join("RESULT.json"), result.to_string()).unwrap();
    if let Ok(out) = std::env::var("NX_SCALE_OUTPUT") {
        let out = std::path::PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("scale-checkpoint.json"), raw).unwrap();
        std::fs::write(
            out.join("scale-graph.blueprint"),
            serde_json::to_vec_pretty(&graph).unwrap(),
        )
        .unwrap();
        std::fs::write(out.join("scale-result.json"), result.to_string()).unwrap();
    }
}
