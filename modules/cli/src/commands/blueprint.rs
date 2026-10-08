//! Blueprint command handlers: save-bp/load-bp/exec/cont/runs/control/say/
//! usage and DSL compile/decompile, plus the blueprint JSON <-> proto
//! conversions (formerly the `bp` module).

use anyhow::Context;
use metteur_proto::proto::daemon_client::DaemonClient;
use metteur_proto::proto::{
    Blueprint, CancelRequest, CompileDslRequest, ContinueExecutionRequest,
    DecompileBlueprintRequest, Edge, ExecuteBlueprintRequest, GetExecutionTreeRequest,
    GetExecutionUsageRequest, InterruptRequest, ListExecutionsRequest, LoadBlueprintRequest, Node,
    PauseRequest, Pin, ResumeRequest, SaveBlueprintRequest,
};
use serde_json::{Value, json};
use tonic::transport::Channel;

use super::*;
use crate::print;

/// Handles `save-bp <file.json> [id]`.
pub(crate) async fn handle_save_bp(
    client: &mut DaemonClient<Channel>,
    state: &SessionState,
    file: String,
    id: Option<String>,
) -> anyhow::Result<Outcome> {
    let ws = require_ws(state)?;
    let text = std::fs::read_to_string(&file)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", file))?;
    let mut blueprint = from_json(&text)?;
    if let Some(id) = &id {
        blueprint = with_id(blueprint, id)?;
    }
    client
        .save_blueprint(SaveBlueprintRequest {
            workspace_path: ws,
            file_path: file.clone(),
            file_json: to_json(&blueprint)?,
            blueprint: Some(blueprint),
        })
        .await
        .map_err(status)?;
    Ok(Outcome::Printed(format!(
        "blueprint saved: {}",
        id.unwrap_or_else(|| "(from file)".to_string())
    )))
}

/// Handles `load-bp <id> [file.json]`.
pub(crate) async fn handle_load_bp(
    client: &mut DaemonClient<Channel>,
    state: &SessionState,
    id: String,
    file: Option<String>,
) -> anyhow::Result<Outcome> {
    let ws = require_ws(state)?;
    let blueprint = client
        .load_blueprint(LoadBlueprintRequest {
            workspace_path: ws,
            blueprint_id: id,
        })
        .await
        .map_err(status)?
        .into_inner();
    let json = to_json(&blueprint)?;
    match file {
        Some(path) => {
            std::fs::write(&path, json)
                .map_err(|e| anyhow::anyhow!("failed to write {}: {e}", path))?;
            Ok(Outcome::Printed(format!("blueprint written to {path}")))
        }
        None => Ok(Outcome::Printed(json)),
    }
}

/// Handles `exec <blueprint_id>`: starts a run and returns its stream.
pub(crate) async fn handle_exec(
    client: &mut DaemonClient<Channel>,
    state: &SessionState,
    blueprint_id: String,
) -> anyhow::Result<Outcome> {
    let ws = require_ws(state)?;
    let known = known_runs(client, &ws).await.unwrap_or_default();
    let stream = client
        .execute_blueprint(ExecuteBlueprintRequest {
            workspace_path: ws.clone(),
            blueprint_id: blueprint_id.clone(),
            // The CLI always runs the stored blueprint; the canvas override is
            // for the GUI client holding an in-memory copy.
            blueprint_json: String::new(),
        })
        .await
        .map_err(status)?
        .into_inner();
    let run_id = discover_run(client, &ws, known, Some(blueprint_id.as_str())).await;
    Ok(Outcome::Started(Box::new(StreamStart {
        stream,
        run_id,
        label: format!("exec {blueprint_id}"),
    })))
}

/// Handles `cont <run_id>`: resumes a suspended run's stream.
pub(crate) async fn handle_cont(
    client: &mut DaemonClient<Channel>,
    state: &SessionState,
    run_id: String,
) -> anyhow::Result<Outcome> {
    let ws = require_ws(state)?;
    let stream = client
        .continue_execution(ContinueExecutionRequest {
            workspace_path: ws.clone(),
            run_id: run_id.clone(),
        })
        .await
        .map_err(status)?
        .into_inner();
    Ok(Outcome::Started(Box::new(StreamStart {
        stream,
        run_id: Some(run_id.clone()),
        label: format!("cont {run_id}"),
    })))
}

/// Handles `runs`: lists executions of the workspace.
pub(crate) async fn handle_runs(
    client: &mut DaemonClient<Channel>,
    state: &SessionState,
) -> anyhow::Result<Outcome> {
    let ws = require_ws(state)?;
    let list = client
        .list_executions(ListExecutionsRequest {
            workspace_path: ws,
        })
        .await
        .map_err(status)?
        .into_inner();
    Ok(Outcome::Printed(print::executions(&list)))
}

/// Handles `tree <run_id>`: shows the agent execution tree of a run.
pub(crate) async fn handle_tree(
    client: &mut DaemonClient<Channel>,
    state: &SessionState,
    run_id: String,
) -> anyhow::Result<Outcome> {
    let ws = require_ws(state)?;
    let tree = client
        .get_execution_tree(GetExecutionTreeRequest {
            workspace_path: ws,
            run_id,
        })
        .await
        .map_err(status)?
        .into_inner();
    Ok(Outcome::Printed(print::execution_tree(&tree)))
}

/// Handles `cancel`.
pub(crate) async fn handle_cancel(
    client: &mut DaemonClient<Channel>,
    state: &SessionState,
) -> anyhow::Result<Outcome> {
    let ws = require_ws(state)?;
    client
        .cancel_execution(CancelRequest {
            run_id: String::new(),
            workspace_path: ws,
        })
        .await
        .map_err(status)?;
    Ok(Outcome::Printed("cancel requested".to_string()))
}

/// Handles `pause`.
pub(crate) async fn handle_pause(
    client: &mut DaemonClient<Channel>,
    state: &SessionState,
) -> anyhow::Result<Outcome> {
    let ws = require_ws(state)?;
    client
        .pause_execution(PauseRequest {
            run_id: String::new(),
            workspace_path: ws,
        })
        .await
        .map_err(status)?;
    Ok(Outcome::Printed("pause requested".to_string()))
}

/// Handles `resume`.
pub(crate) async fn handle_resume(
    client: &mut DaemonClient<Channel>,
    state: &SessionState,
) -> anyhow::Result<Outcome> {
    let ws = require_ws(state)?;
    client
        .resume_execution(ResumeRequest {
            run_id: String::new(),
            workspace_path: ws,
        })
        .await
        .map_err(status)?;
    Ok(Outcome::Printed("resume requested".to_string()))
}

/// Handles `say <priority> <message>`: sends an interrupt.
pub(crate) async fn handle_say(
    client: &mut DaemonClient<Channel>,
    state: &SessionState,
    priority: String,
    message: String,
) -> anyhow::Result<Outcome> {
    let ws = require_ws(state)?;
    client
        .send_interrupt(InterruptRequest {
            workspace_path: ws,
            priority: priority.clone(),
            message: message.clone(),
        })
        .await
        .map_err(status)?;
    Ok(Outcome::Printed(format!("interrupt sent ({priority}): {message}")))
}

/// Handles `usage <run_id>`.
pub(crate) async fn handle_usage(
    client: &mut DaemonClient<Channel>,
    state: &SessionState,
    run_id: String,
) -> anyhow::Result<Outcome> {
    let ws = require_ws(state)?;
    let summary = client
        .get_execution_usage(GetExecutionUsageRequest {
            workspace_path: ws,
            run_id,
        })
        .await
        .map_err(status)?
        .into_inner();
    Ok(Outcome::Printed(print::usage(&summary)))
}

/// Handles `bp compile <file.mbp> [save [as <id>]]`.
pub(crate) async fn handle_bp_compile(
    client: &mut DaemonClient<Channel>,
    state: &SessionState,
    file: String,
    save: bool,
    save_to: Option<String>,
) -> anyhow::Result<Outcome> {
    let ws = require_ws(state)?;
    let source = std::fs::read_to_string(&file)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", file))?;
    let blueprint = client
        .compile_dsl(CompileDslRequest {
            workspace_path: ws.clone(),
            source,
        })
        .await
        .map_err(status)?
        .into_inner();
    if save {
        let blueprint = match save_to {
            Some(id) => with_id(blueprint, &id)?,
            None => blueprint,
        };
        let id = blueprint.id.clone();
        client
            .save_blueprint(SaveBlueprintRequest {
                workspace_path: ws,
                file_path: format!("{file}.blueprint"),
                file_json: to_json(&blueprint)?,
                blueprint: Some(blueprint),
            })
            .await
            .map_err(status)?;
        Ok(Outcome::Printed(format!("blueprint compiled and saved as {id} at {file}.blueprint")))
    } else {
        Ok(Outcome::Printed(to_json(&blueprint)?))
    }
}

/// Handles `bp decompile <blueprint_id>`.
pub(crate) async fn handle_bp_decompile(
    client: &mut DaemonClient<Channel>,
    state: &SessionState,
    id: String,
) -> anyhow::Result<Outcome> {
    let ws = require_ws(state)?;
    let resp = client
        .decompile_blueprint(DecompileBlueprintRequest {
            workspace_path: ws,
            blueprint_id: id.clone(),
            blueprint: None,
        })
        .await
        .map_err(status)?
        .into_inner();
    Ok(Outcome::Printed(resp.source))
}

/// Best-effort snapshot of run ids already known to the daemon.
async fn known_runs(client: &mut DaemonClient<Channel>, ws: &str) -> anyhow::Result<Vec<String>> {
    let list = client
        .list_executions(ListExecutionsRequest {
            workspace_path: ws.to_string(),
        })
        .await
        .map_err(status)?
        .into_inner();
    Ok(list.executions.into_iter().map(|e| e.run_id).collect())
}

/// Finds the newly created run for `blueprint_id`, if its checkpoint exists yet.
async fn discover_run(
    client: &mut DaemonClient<Channel>,
    ws: &str,
    known: Vec<String>,
    blueprint_id: Option<&str>,
) -> Option<String> {
    let list = client
        .list_executions(ListExecutionsRequest {
            workspace_path: ws.to_string(),
        })
        .await
        .ok()?
        .into_inner();
    list.executions
        .into_iter()
        .find(|e| !known.contains(&e.run_id) && blueprint_id.is_none_or(|b| e.blueprint_id == b))
        .map(|e| e.run_id)
}

// --- Blueprint JSON <-> proto conversions (from the former `bp` module). ---
//
// The on-disk format mirrors `metteur_shared::Blueprint`: snake_case fields,
// node positions as `[x, y]` pairs and node data embedded as raw JSON.

/// Parses a blueprint JSON document into its proto form.
pub(crate) fn from_json(text: &str) -> anyhow::Result<Blueprint> {
    let doc: Value = serde_json::from_str(text).context("blueprint file is not valid JSON")?;
    let mut blueprint = Blueprint {
        id: uuid_field(&doc, "id")?,
        name: str_field(&doc, "name"),
        entry_node_id: uuid_field(&doc, "entry_node_id")?,
        nodes: doc
            .get("nodes")
            .and_then(Value::as_array)
            .map(|nodes| nodes.iter().map(parse_node).collect())
            .transpose()?
            .unwrap_or_default(),
        edges: doc
            .get("edges")
            .and_then(Value::as_array)
            .map(|edges| edges.iter().map(parse_edge).collect())
            .transpose()?
            .unwrap_or_default(),
    };
    let nulls = doc["explicit_null_defaults"].as_array();
    for pin in blueprint.nodes.iter_mut().flat_map(|n| &mut n.pins) {
        if nulls.is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(&pin.id))) {
            pin.default_json = "null".into();
        }
    }
    Ok(blueprint)
}

/// Renders a proto blueprint as pretty JSON text.
pub(crate) fn to_json(bp: &Blueprint) -> anyhow::Result<String> {
    serde_json::to_string_pretty(&json!({
        "id": bp.id,
        "name": bp.name,
        "entry_node_id": bp.entry_node_id,
        "explicit_null_defaults": bp.nodes.iter().flat_map(|n| &n.pins).filter(|p| p.default_json == "null").map(|p| &p.id).collect::<Vec<_>>(),
        "nodes": bp.nodes.iter().map(node_value).collect::<Vec<_>>(),
        "edges": bp.edges.iter().map(edge_value).collect::<Vec<_>>(),
    }))
    .context("failed to serialize blueprint")
}

/// Overrides the blueprint id after validating it is a UUID.
pub(crate) fn with_id(mut bp: Blueprint, id: &str) -> anyhow::Result<Blueprint> {
    uuid::Uuid::parse_str(id).context("blueprint id must be a UUID")?;
    bp.id = id.to_string();
    Ok(bp)
}

fn parse_node(node: &Value) -> anyhow::Result<Node> {
    let data = node.get("data").cloned().unwrap_or_else(|| json!({}));
    let position =
        node.get("position").and_then(Value::as_array).map(|p| (num(p.first()), num(p.get(1))));
    let pins = node
        .get("pins")
        .and_then(Value::as_array)
        .map(|pins| pins.iter().map(parse_pin).collect())
        .transpose()?
        .unwrap_or_default();
    Ok(Node {
        id: uuid_field(node, "id")?,
        node_type: str_field(node, "node_type"),
        kind: str_field(node, "kind"),
        pos_x: position.unwrap_or((0.0, 0.0)).0,
        pos_y: position.unwrap_or((0.0, 0.0)).1,
        pins,
        data_json: serde_json::to_string(&data).context("invalid node data")?,
    })
}

fn parse_pin(pin: &Value) -> anyhow::Result<Pin> {
    Ok(Pin {
        id: uuid_field(pin, "id")?,
        name: str_field(pin, "name"),
        pin_type: str_field(pin, "pin_type"),
        data_type: str_field(pin, "data_type"),
        key: str_field(pin, "key"),
        default_json: pin
            .get("default")
            .filter(|v| !v.is_null())
            .map(Value::to_string)
            .unwrap_or_default(),
        optional: pin["optional"].as_bool().unwrap_or(false),
        choices: pin["choices"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        description: str_field(pin, "description"),
    })
}

fn parse_edge(edge: &Value) -> anyhow::Result<Edge> {
    Ok(Edge {
        id: uuid_field(edge, "id")?,
        source_node: uuid_field(edge, "source_node")?,
        source_pin: uuid_field(edge, "source_pin")?,
        target_node: uuid_field(edge, "target_node")?,
        target_pin: uuid_field(edge, "target_pin")?,
    })
}

/// Reads a string field that must be a UUID.
fn uuid_field(obj: &Value, field: &str) -> anyhow::Result<String> {
    let value = obj
        .get(field)
        .and_then(Value::as_str)
        .with_context(|| format!("missing blueprint field '{field}'"))?;
    uuid::Uuid::parse_str(value)
        .with_context(|| format!("blueprint field '{field}' is not a UUID"))?;
    Ok(value.to_string())
}

fn str_field(obj: &Value, field: &str) -> String {
    obj.get(field).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn num(value: Option<&Value>) -> f32 {
    value.and_then(Value::as_f64).unwrap_or(0.0) as f32
}

fn node_value(node: &Node) -> Value {
    json!({
        "id": node.id,
        "node_type": node.node_type,
        "kind": node.kind,
        "position": [node.pos_x, node.pos_y],
        "pins": node.pins.iter().map(|p| json!({
            "id": p.id,
            "name": p.name,
            "pin_type": p.pin_type,
            "data_type": p.data_type,
            "key": if p.key.is_empty() { None } else { Some(&p.key) },
            "default": serde_json::from_str::<Value>(&p.default_json).ok(),
            "optional": p.optional, "choices": p.choices,
            "description": if p.description.is_empty() { None } else { Some(&p.description) },
        })).collect::<Vec<_>>(),
        "data": serde_json::from_str::<Value>(&node.data_json).unwrap_or(Value::Null),
    })
}

fn edge_value(edge: &Edge) -> Value {
    json!({
        "id": edge.id,
        "source_node": edge.source_node,
        "source_pin": edge.source_pin,
        "target_node": edge.target_node,
        "target_pin": edge.target_pin,
    })
}

/// Reads only the daemon's redacted projection, with no implicit file lookup.
pub(crate) async fn handle_blackboard(
    client: &mut DaemonClient<Channel>,
    state: &SessionState,
    run_id: String,
    query_json: String,
) -> anyhow::Result<Outcome> {
    let result = client
        .get_blackboard(metteur_proto::proto::GetBlackboardRequest {
            workspace_path: require_ws(state)?,
            run_id,
            query_json,
        })
        .await
        .map_err(status)?
        .into_inner();
    let value: serde_json::Value = serde_json::from_str(&result.projection_json)?;
    Ok(Outcome::Printed(serde_json::to_string_pretty(&value)?))
}

#[cfg(test)]
mod file_tests {
    use super::*;
    #[test]
    fn native_file_preserves_pin_metadata_and_explicit_null() {
        let graph = Blueprint {
            id: uuid::Uuid::new_v4().to_string(),
            name: "pins".into(),
            entry_node_id: uuid::Uuid::new_v4().to_string(),
            nodes: vec![Node {
                id: uuid::Uuid::new_v4().to_string(),
                pins: vec![Pin {
                    id: uuid::Uuid::new_v4().to_string(),
                    name: "Value".into(),
                    key: "value".into(),
                    pin_type: "DataInput".into(),
                    data_type: "any".into(),
                    default_json: "null".into(),
                    optional: true,
                    choices: vec!["one".into()],
                    description: "Keep this".into(),
                }],
                data_json: "{}".into(),
                ..Default::default()
            }],
            edges: vec![],
        };
        let restored = from_json(&to_json(&graph).unwrap()).unwrap();
        assert_eq!(restored, graph);
    }
}
