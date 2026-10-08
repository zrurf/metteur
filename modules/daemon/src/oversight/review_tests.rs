use super::blueprint_nodes;
use crate::execution::ExecutionCheckpoint;
use metteur_shared::Blueprint;
use serde_json::json;
use uuid::Uuid;

fn graph() -> Blueprint {
    metteur_shared::dsl::compile_draft_value_with_catalog(
        &json!({"name":"Inspection","nodes":{"s":{"kind":"Start"},"d":{"kind":"Delay","Ms":1000},"e":{"kind":"End"}},"flow":["s -> d -> e"]}),
        &crate::registry::Registry::with_builtins().authoring_catalog(),
    ).unwrap()
}

#[test]
fn supervisor_can_inspect_the_root_before_the_first_node_runs() {
    let bp = graph();
    let mut cp = ExecutionCheckpoint::running(Uuid::new_v4(), bp.id, 0);
    cp.view.root = Some(bp.clone());
    assert!(cp.view.graphs.is_empty());
    let nodes = blueprint_nodes(&cp);
    assert_eq!(nodes.len(), bp.nodes.len());
    for (actual, expected) in nodes.iter().zip(&bp.nodes) {
        assert_eq!(actual["scope"], json!(bp.id));
        assert_eq!(actual["node"], json!(expected));
    }
}

#[test]
fn supervisor_reads_the_effective_root_instead_of_its_previous_invocation_image() {
    let previous = graph();
    let mut current = previous.clone();
    current.nodes.iter_mut().find(|n| n.kind == "Delay").unwrap().data = json!({"Ms":2000});
    let mut nested = graph();
    nested.id = Uuid::new_v4();
    let mut cp = ExecutionCheckpoint::running(Uuid::new_v4(), current.id, 0);
    cp.view.root = Some(current.clone());
    cp.view.graphs.insert(previous.id.to_string(), previous);
    cp.view.graphs.insert(nested.id.to_string(), nested.clone());
    let nodes = blueprint_nodes(&cp);
    assert_eq!(nodes.len(), current.nodes.len() + nested.nodes.len());
    let root: Vec<_> = nodes
        .iter()
        .filter(|n| n["scope"] == json!(current.id))
        .map(|n| n["node"].clone())
        .collect();
    assert_eq!(root, current.nodes.iter().map(|n| json!(n)).collect::<Vec<_>>());
    assert!(nodes.iter().any(|n| n["scope"] == json!(nested.id)));
    assert!(
        nodes.iter().filter(|n| n["scope"] == json!(nested.id)).all(|n| n["edit_match"].is_null())
    );
}

#[test]
fn copied_edit_match_changes_only_the_second_root_delay() {
    let mut bp = graph();
    let mut second = bp.nodes.iter().find(|n| n.kind == "Delay").unwrap().clone();
    second.id = Uuid::new_v4();
    second.data = json!({"ms":3000,"retained":"unchanged"});
    let target = second.id;
    bp.nodes.push(second);
    let mut cp = ExecutionCheckpoint::running(Uuid::new_v4(), bp.id, 0);
    cp.view.root = Some(bp.clone());
    let nodes = blueprint_nodes(&cp);
    let node = nodes.iter().find(|n| n["node"]["id"] == json!(target)).unwrap();
    let key = node["node"]["pins"].as_array().unwrap().iter().find(|p| p["name"] == "Ms").unwrap()
        ["key"]
        .clone();
    let before = bp.clone();
    crate::replan::apply_edits(
        &mut bp,
        &json!([{
            "op":"set_pin","match":node["edit_match"],"pin":key,"value":2000
        }]),
    )
    .unwrap();
    for (old, new) in before.nodes.iter().zip(&bp.nodes) {
        if new.id == target {
            assert_eq!(new.data, json!({"ms":2000,"retained":"unchanged"}));
        } else {
            assert_eq!(old, new);
        }
    }
}
