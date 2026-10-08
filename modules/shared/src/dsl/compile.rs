//! Compiles parsed statements into a [`Blueprint`].
//!
//! Node pin layouts come from a supplied registry snapshot. Offline callers
//! may use the built-in contracts adopted by the daemon's built-in executors.
//! Errors carry the source line/column recorded by the parser.

use std::collections::{HashMap, VecDeque};

use uuid::Uuid;

use crate::error::{SharedError, SharedResult};
use crate::model::blueprint::{Blueprint, DataType, Edge, Node, Pin, PinType};

use super::parser::Statement;
use crate::node_catalog::{NodeCatalog, builtin_catalog};

/// Maximum nodes a compiled blueprint may contain (arbitrary safety limit).
const MAX_NODES: usize = 256;

/// Folds the DSL's flat retry args (`retry_max_attempts`, `retry_rollback`)
/// into the nested `retry` object the interpreter reads, merging with an
/// explicitly written `retry` object when both are present.
fn fold_validator_retry(data: &mut serde_json::Map<String, serde_json::Value>) {
    let flat = [("retry_max_attempts", "max_attempts"), ("retry_rollback", "rollback")];
    let mut retry = data.get("retry").and_then(|v| v.as_object()).cloned().unwrap_or_default();
    let mut changed = false;
    for (flat_key, nested_key) in flat {
        if let Some(v) = data.remove(flat_key) {
            retry.insert(nested_key.to_string(), v);
            changed = true;
        }
    }
    if changed {
        data.insert("retry".to_string(), serde_json::Value::Object(retry));
    }
}

/// Compiles `source` into a blueprint with fresh UUIDs.
pub fn compile(source: &str) -> SharedResult<Blueprint> {
    compile_with_catalog(source, &builtin_catalog())
}

/// Compiles against an immutable snapshot of the daemon registry.
pub fn compile_with_catalog(source: &str, catalog: &NodeCatalog) -> SharedResult<Blueprint> {
    let statements = super::parser::parse(source)?;
    let mut name = "Untitled".to_string();
    let mut nodes: Vec<&Statement> = Vec::new();
    let mut exec_edges: Vec<&Statement> = Vec::new();
    let mut data_wires: Vec<&Statement> = Vec::new();

    for stmt in &statements {
        match stmt {
            Statement::Header {
                name: n,
            } => name = n.clone(),
            Statement::Node {
                ..
            } => nodes.push(stmt),
            Statement::ExecEdge {
                ..
            } => exec_edges.push(stmt),
            Statement::DataWire {
                ..
            } => data_wires.push(stmt),
        }
    }

    if nodes.is_empty() {
        return Err(SharedError::Invalid("no nodes declared".to_string()));
    }
    if nodes.len() > MAX_NODES {
        return Err(SharedError::Invalid(format!(
            "too many nodes ({} > {MAX_NODES})",
            nodes.len()
        )));
    }

    let entry = nodes
        .iter()
        .copied()
        .find(|s| {
            matches!(
                s,
                Statement::Node {
                    is_entry: true,
                    ..
                }
            )
        })
        .unwrap_or(nodes[0]);
    let entry_alias = match entry {
        Statement::Node {
            alias,
            ..
        } => alias.as_str(),
        _ => "",
    };

    let mut blueprint = Blueprint {
        id: Uuid::new_v4(),
        name,
        nodes: Vec::new(),
        edges: Vec::new(),
        entry_node_id: Uuid::nil(),
    };
    let node_id_of: HashMap<&str, Uuid> = nodes
        .iter()
        .map(|s| match s {
            Statement::Node {
                alias,
                ..
            } => (alias.as_str(), Uuid::new_v4()),
            _ => unreachable!(),
        })
        .collect();
    let mut src_pin_of: HashMap<(String, String), (Uuid, Uuid)> = HashMap::new();

    for node_stmt in &nodes {
        let Statement::Node {
            alias,
            kind,
            constants,
            line,
            col,
            ..
        } = *node_stmt
        else {
            unreachable!("nodes only contains Node statements")
        };
        let node_id = node_id_of[alias.as_str()];
        if alias.as_str() == entry_alias {
            blueprint.entry_node_id = node_id;
        }
        let signature = catalog.resolve(kind, &serde_json::Value::Object(constants.iter().cloned().collect())).ok_or_else(|| {
            SharedError::Invalid(format!(
                "unknown node kind '{kind}' (line {line}, column {col}); use a JSON blueprint instead"
            ))
        })?;
        let spec = &signature.pins;
        let mut pins = Vec::new();
        for p in spec {
            let pin = p.instantiate(Uuid::new_v4());
            if p.pin_type == PinType::DataOutput {
                src_pin_of.insert((alias.clone(), p.name.clone()), (node_id, pin.id));
            }
            pins.push(pin);
        }
        // Switch case branches are declared with `cases = [...]`; each entry
        // becomes a `Case_<value>` execution output pin.
        if kind == "Switch"
            && let Some(cases) =
                constants.iter().find(|(k, _)| k == "cases").and_then(|(_, v)| v.as_array())
        {
            for case in cases {
                let Some(text) = case.as_str() else {
                    return Err(SharedError::Invalid(format!(
                        "switch cases must be strings (line {line}, column {col})"
                    )));
                };
                let name = format!("Case_{text}");
                if pins.iter().any(|pin: &Pin| pin.name == name) {
                    continue;
                }
                pins.push(Pin::data(name, PinType::ExecOutput, DataType::Void, Uuid::new_v4()));
            }
        }
        // Start carries ad-hoc initial attributes: constants outside the
        // template become data output pins so downstream nodes can reference
        // them (`A <- start.A`). The Start executor emits data values by pin
        // name, matching the stored constant keys.
        if kind == "Start" {
            for (k, _) in constants {
                let known = spec.iter().any(|p| p.key == *k || p.name == *k)
                    || pins.iter().any(|pin: &Pin| &pin.name == k);
                if !known {
                    let id = Uuid::new_v4();
                    src_pin_of.insert((alias.clone(), k.clone()), (node_id, id));
                    pins.push(Pin {
                        id,
                        key: Some(k.clone()),
                        name: k.clone(),
                        pin_type: PinType::DataOutput,
                        data_type: DataType::Any,
                        ..Default::default()
                    });
                }
            }
        }
        let mut data = serde_json::Map::new();
        for (k, v) in constants {
            // Store constants under the pin's canonical key when one exists, so
            // executor lookups and decompile agree whichever spelling the DSL
            // used (`A` in parens vs `a` in a block).
            let key = spec
                .iter()
                .find(|p| p.key == *k || p.name == *k)
                .map(|p| p.key.as_str())
                .filter(|k| !k.is_empty())
                .unwrap_or(k.as_str());
            data.insert(key.to_string(), v.clone());
        }
        if kind == "Validator" {
            fold_validator_retry(&mut data);
        }
        if let Some(binding) = catalog.addon_bindings.get(kind) {
            let key = crate::node_catalog::addon::BINDING_KEY;
            if data.get(key).is_some_and(|old| old != binding) {
                return Err(SharedError::Invalid("Addon package changed; recreate the node explicitly".into()));
            }
            data.insert(key.into(), binding.clone());
        }
        if kind == "CallFunction" && let Some(binding)=data.get("function").and_then(|v|v.as_str()).and_then(|name|catalog.addon_function_bindings.get(name)) {
            let key=crate::node_catalog::addon::FUNCTION_BINDING_KEY;
            if data.get(key).is_some_and(|old|old!=binding) {return Err(SharedError::Invalid("Addon function or dependency changed; recreate the call explicitly".into()));}
            data.insert(key.into(),binding.clone());
        }
        // The three Round 13 search/edit tools are ordinary registry tools, so
        // the DSL spells them by name and the compiler emits a `Tool` node with
        // the matching `tool_name`; everything else about the node is generic.
        let node_kind = signature.executor_kind.as_str();
        let tool_name = (node_kind == "Tool" && kind != "Tool").then(|| kind.clone());
        if let Some(tool_name) = tool_name {
            data.insert("tool_name".to_string(), serde_json::Value::String(tool_name));
        }
        blueprint.nodes.push(Node {
            id: node_id,
            node_type: signature.node_type,
            kind: node_kind.to_string(),
            position: (0.0, 0.0),
            pins,
            data: serde_json::Value::Object(data),
        });
    }

    // Data refs written inline inside node args (`pin <- alias.pin`).
    for node_stmt in &nodes {
        let Statement::Node {
            alias,
            refs,
            line,
            col,
            ..
        } = *node_stmt
        else {
            unreachable!("nodes only contains Node statements")
        };
        for (target_pin, source) in refs {
            let (source_alias, source_pin) = source
                .split_once('.')
                .ok_or_else(|| SharedError::Invalid(format!("invalid ref '{source}'")))?;
            let (source_node, source_pin_id) = src_pin_of
                .get(&(source_alias.to_string(), source_pin.to_string()))
                .cloned()
                .ok_or_else(|| {
                    SharedError::Invalid(format!(
                        "unknown data source '{source}' (node {source_alias} has no data pin '{source_pin}')"
                    ))
                })?;
            let target_node = *node_id_of
                .get(alias.as_str())
                .ok_or_else(|| SharedError::Invalid(format!("unknown node '{alias}'")))?;
            let target_pin_id = find_data_input(&blueprint, target_node, target_pin).map_err(
                |_| {
                    SharedError::Invalid(format!(
                        "node '{alias}' has no data input pin '{target_pin}' (line {line}, column {col})"
                    ))
                },
            )?;
            blueprint.edges.push(Edge {
                id: Uuid::new_v4(),
                source_node,
                source_pin: source_pin_id,
                target_node,
                target_pin: target_pin_id,
            });
        }
    }

    // Top-level data wires plus exec edges.
    for wire in data_wires {
        let Statement::DataWire {
            target,
            source,
            line,
            col,
        } = wire
        else {
            unreachable!("data_wires only contains DataWire statements")
        };
        let (source_node, source_pin_id) =
            src_pin_of.get(&(source.0.clone(), source.1.clone())).cloned().ok_or_else(|| {
                SharedError::Invalid(format!(
                    "unknown data source '{}.{}' (line {line}, column {col})",
                    source.0, source.1
                ))
            })?;
        let target_node = *node_id_of
            .get(target.0.as_str())
            .ok_or_else(|| SharedError::Invalid(format!("unknown node '{}'", target.0)))?;
        let target_pin_id = find_data_input(&blueprint, target_node, &target.1).map_err(|_| {
            SharedError::Invalid(format!(
                "node '{}' has no data input pin '{}' (line {line}, column {col})",
                target.0, target.1
            ))
        })?;
        blueprint.edges.push(Edge {
            id: Uuid::new_v4(),
            source_node,
            source_pin: source_pin_id,
            target_node,
            target_pin: target_pin_id,
        });
    }
    for edge in exec_edges {
        let Statement::ExecEdge {
            source,
            source_pin,
            target,
            line,
            col,
        } = edge
        else {
            unreachable!("exec_edges only contains ExecEdge statements")
        };
        let source_node = *node_id_of.get(source.as_str()).ok_or_else(|| {
            SharedError::Invalid(format!("unknown node '{source}' (line {line}, column {col})"))
        })?;
        let source_pin_id = {
            let node = blueprint.nodes.iter().find(|n| n.id == source_node).unwrap();
            match source_pin {
                Some(pin_name) => node
                    .pins
                    .iter()
                    .find(|p| p.name == *pin_name && p.pin_type == PinType::ExecOutput)
                    .map(|p| p.id)
                    .ok_or_else(|| {
                        SharedError::Invalid(format!(
                            "node '{source}' has no exec output '{pin_name}' (line {line}, column {col})"
                        ))
                    })?,
                None => node
                    .pins
                    .iter()
                    .find(|p| p.pin_type == PinType::ExecOutput)
                    .map(|p| p.id)
                    .ok_or_else(|| {
                        SharedError::Invalid(format!(
                            "node '{source}' has no exec output (line {line}, column {col})"
                        ))
                    })?,
            }
        };
        let target_node = *node_id_of.get(target.as_str()).ok_or_else(|| {
            SharedError::Invalid(format!("unknown node '{target}' (line {line}, column {col})"))
        })?;
        let target_pin_id = blueprint
            .nodes
            .iter()
            .find(|n| n.id == target_node)
            .and_then(|n| n.pins.iter().find(|p| p.pin_type == PinType::ExecInput))
            .map(|p| p.id)
            .ok_or_else(|| {
                SharedError::Invalid(format!(
                    "node '{target}' has no exec input (line {line}, column {col})"
                ))
            })?;
        blueprint.edges.push(Edge {
            id: Uuid::new_v4(),
            source_node,
            source_pin: source_pin_id,
            target_node,
            target_pin: target_pin_id,
        });
    }
    // Lay the graph out along the exec flow so imported blueprints read as a
    // cascade instead of one overlapping row.
    lay_out(&mut blueprint);
    Ok(blueprint)
}

/// Positions nodes in a layered grid: columns are exec-flow depth from the
/// entry node, rows keep the declaration order within a column.
fn lay_out(bp: &mut Blueprint) {
    let exec_next: Vec<(Uuid, Uuid)> = bp
        .edges
        .iter()
        .filter_map(|e| {
            let src = bp.nodes.iter().find(|n| n.id == e.source_node)?;
            let dst = bp.nodes.iter().find(|n| n.id == e.target_node)?;
            let is_exec_out =
                src.pins.iter().any(|p| p.id == e.source_pin && p.pin_type == PinType::ExecOutput);
            let is_exec_in =
                dst.pins.iter().any(|p| p.id == e.target_pin && p.pin_type == PinType::ExecInput);
            (is_exec_out && is_exec_in).then_some((e.source_node, e.target_node))
        })
        .collect();

    let mut layer_of: HashMap<Uuid, u32> = HashMap::new();
    let mut queue = VecDeque::new();
    layer_of.insert(bp.entry_node_id, 0);
    queue.push_back(bp.entry_node_id);
    while let Some(nid) = queue.pop_front() {
        let cur = layer_of[&nid];
        for &(s, t) in &exec_next {
            if s == nid && !layer_of.contains_key(&t) {
                layer_of.insert(t, cur + 1);
                queue.push_back(t);
            }
        }
    }
    // Nodes not reachable through the exec flow (data-only parts, detached
    // graphs) continue after the deepest exec layer.
    let mut fallback = layer_of.len() as u32;
    for n in &bp.nodes {
        layer_of.entry(n.id).or_insert_with(|| {
            fallback += 1;
            fallback - 1
        });
    }

    let mut column: HashMap<u32, u32> = HashMap::new();
    for n in &mut bp.nodes {
        let layer = layer_of[&n.id];
        let row = column.entry(layer).or_insert(0);
        n.position = (layer as f32 * 260.0 + 40.0, *row as f32 * 170.0 + 60.0);
        *row += 1;
    }
}

/// Reads a data input pin by name, verifying it is a `DataInput`.
fn find_data_input(bp: &Blueprint, node_id: Uuid, pin_name: &str) -> SharedResult<Uuid> {
    bp.nodes
        .iter()
        .find(|n| n.id == node_id)
        .and_then(|n| {
            n.pins.iter().find(|p| p.name == pin_name && p.pin_type == PinType::DataInput)
        })
        .map(|p| p.id)
        .ok_or_else(|| SharedError::Invalid(format!("node has no data input pin '{pin_name}'")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::decompile;

    /// A context-manager graph as exported by the canvas must recompile and
    /// keep its Context wiring (regression: the template table named context
    /// pins literally `DataType::Context` and Start lacked the Context output).
    #[test]
    fn context_blueprint_round_trips() {
        let source = r#"blueprint "abc.blueprint"
entry n0: Start
n1: End
n2: CallLLM
n3: ContextMerge
n4: ContextTrim
n2.x-out -> n3
$n3.Text <- n2.Result
$n3.Context <- n2.Context
n0.x-out -> n2
$n2.Context <- n0.Context
$n4.Context <- n3.Result
n3.x-out -> n4
n4.x-out -> n1
"#;
        let bp = compile(source).expect("canvas-derived DSL must compile");
        assert_eq!(bp.entry_node_id, bp.nodes[0].id);
        let out = decompile(&bp);
        assert!(out.contains("$n2.Context <- n0.Context"), "{out}");
        assert!(out.contains("$n4.Context <- n3.Result"), "{out}");
    }

    /// Exec pins use the canvas `x-in`/`x-out` names, so exported edges with
    /// explicit exec suffixes resolve after a round trip.
    #[test]
    fn exec_suffix_round_trips() {
        let source = "entry n0: Start\nn1: Add\nn2: End\nn0.x-out -> n1\nn1.x-out -> n2\n";
        let bp = compile(source).expect("suffixed exec edges must compile");
        let out = decompile(&bp);
        assert!(out.contains("n0.x-out -> n1"), "{out}");
    }

    /// Inline node constants (literals) survive a compile -> decompile round
    /// trip, so values filled in the inspector are not lost on export. Both the
    /// paren form and the `{ key: value }` block form compile; constants are
    /// stored (and re-emitted) under the pin's canonical key.
    #[test]
    fn constants_survive_decompile() {
        let source = "entry n0: Add(A = 8, B = 3)\nn1: End\nn0.x-out -> n1\n";
        let bp = compile(source).expect("constants must compile");
        assert_eq!(bp.nodes[0].data.get("a"), Some(&serde_json::json!(8)));
        let out = decompile(&bp);
        assert!(out.contains("a: 8"), "{out}");
        assert!(out.contains("b: 3"), "{out}");
    }

    /// The init-block syntax (`n: Kind { key: literal }`) stores node constants
    /// and survives a decompile round trip as the same block.
    #[test]
    fn init_block_stores_constants() {
        let source = "entry n0: CallLLM {\n  max_tokens: 256000\n  prompt: \"测试\"\n}\nn1: End\nn0.x-out -> n1\n";
        let bp = compile(source).expect("init block must compile");
        let node = &bp.nodes[0];
        assert_eq!(node.data.get("max_tokens"), Some(&serde_json::json!(256000)));
        assert_eq!(node.data.get("prompt"), Some(&serde_json::json!("测试")));
        let out = decompile(&bp);
        assert!(out.contains("max_tokens: 256000"), "{out}");
        assert!(out.contains("prompt: \"测试\""), "{out}");
    }

    /// Positions follow the exec flow: deeper layers move right, siblings in
    /// the same layer stack vertically instead of overlapping.
    #[test]
    fn layout_follows_exec_flow() {
        let source = "\
entry n0: Start
n1: Add
n2: Length
n3: End
n0.x-out -> n1
n0.x-out -> n2
n1.x-out -> n3
n2.x-out -> n3
";
        let bp = compile(source).expect("branching graph must compile");
        let by = |kind: &str| bp.nodes.iter().find(|n| n.kind == kind).expect(kind);
        assert!(by("Start").position.0 < by("Add").position.0);
        assert!(by("Add").position.0 < by("End").position.0);
        // Layer-1 siblings (Add, Length) occupy distinct rows.
        assert_ne!(by("Add").position.1, by("Length").position.1);
    }
}
