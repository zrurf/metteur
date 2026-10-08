//! Static validation of a blueprint graph.
//!
//! Without this, a malformed graph is only discovered when the offending node
//! executes — after the checkpoint machinery, the LLM budget and any file
//! mutations of the preceding nodes are already committed. The visual editor
//! makes such graphs easy to produce, so the checks run before a blueprint is
//! stored and again before it executes.
//!
//! Only structural and type compatibility are checked here. Semantic
//! questions ("does this branch make sense?") stay with the LLM.

use std::collections::HashSet;
use std::fmt;

use super::blueprint::{Blueprint, DataType, Edge, NodeId, PinType};
use super::types::compatible;

/// A single reason a blueprint is invalid, anchored to the node that caused it.
#[derive(Debug, Clone, PartialEq)]
pub enum BlueprintError {
    /// A package node no longer matches its immutable declared contract.
    AddonContract { node_id: NodeId, detail: String },
    /// The entry node id does not resolve to a node in the graph.
    MissingEntryNode(NodeId),
    /// An edge names a node that is not in the graph.
    UnknownEdgeNode {
        /// The edge with the dangling reference.
        edge: Edge,
    },
    /// An edge names a pin that does not exist.
    UnknownPin {
        /// The edge with the dangling pin.
        edge: Edge,
    },
    /// The pin exists, but belongs to a different node than the edge claims.
    PinNotOnNode {
        /// The edge whose pin ownership is wrong.
        edge: Edge,
        /// The node the pin actually belongs to.
        owner: NodeId,
    },
    /// An execution pin was wired to a data pin, or the reverse.
    PinKindMismatch {
        /// The offending edge.
        edge: Edge,
        /// The source pin's role.
        source_kind: PinType,
        /// The target pin's role.
        target_kind: PinType,
    },
    /// Two execution outputs wired together, or two execution inputs.
    ExecPinDirection {
        /// The offending edge.
        edge: Edge,
    },
    /// A data edge connects types that cannot flow into each other.
    IncompatibleTypes {
        /// The offending edge.
        edge: Edge,
        /// The data type produced by the source pin.
        source: DataType,
        /// The data type the target pin accepts.
        target: DataType,
    },
    /// A required data input is neither connected nor given a default.
    UnconnectedRequiredInput {
        /// The node holding the input.
        node_id: NodeId,
        /// The node's kind, for a readable message.
        kind: String,
        /// The input pin that has no source.
        pin: String,
    },
    /// The execution graph contains a cycle.
    ExecCycle,
}

impl fmt::Display for BlueprintError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AddonContract { node_id, detail } => write!(f,"addon node {node_id}: {detail}"),
            Self::MissingEntryNode(id) => {
                write!(f, "entry node {id} is not part of the blueprint")
            }
            Self::UnknownEdgeNode {
                edge,
            } => write!(
                f,
                "edge {} references node {} or {}, which does not exist",
                edge.id, edge.source_node, edge.target_node
            ),
            Self::UnknownPin {
                edge,
            } => write!(
                f,
                "edge {} references pin {} or {}, which does not exist",
                edge.id, edge.source_pin, edge.target_pin
            ),
            Self::PinNotOnNode {
                edge,
                owner,
            } => write!(
                f,
                "edge {} claims pin {} belongs to node {}, but it belongs to node {owner}",
                edge.id, edge.source_pin, edge.source_node
            ),
            Self::PinKindMismatch {
                edge,
                source_kind,
                target_kind,
            } => write!(
                f,
                "edge {} connects {source_kind:?} pin {} to {target_kind:?} pin {}: \
                 execution and data pins cannot be mixed",
                edge.id, edge.source_pin, edge.target_pin
            ),
            Self::ExecPinDirection {
                edge,
            } => write!(
                f,
                "edge {} connects execution pin {} to execution pin {}: \
                 an execution output must feed an execution input",
                edge.id, edge.source_pin, edge.target_pin
            ),
            Self::IncompatibleTypes {
                edge,
                source,
                target,
            } => write!(
                f,
                "edge {} carries {source} into a {target} pin ({} -> {}); \
                 insert an explicit conversion node such as ToInt",
                edge.id, edge.source_pin, edge.target_pin
            ),
            Self::UnconnectedRequiredInput {
                node_id,
                kind,
                pin,
            } => write!(
                f,
                "node {node_id} ({kind}) requires input '{pin}' but it is unconnected \
                 and has no default"
            ),
            Self::ExecCycle => write!(f, "the execution graph contains a cycle"),
        }
    }
}

impl std::error::Error for BlueprintError {}

/// Validates a blueprint, collecting every problem found.
/// The result of validating one blueprint.
///
/// `errors` block a save or a run; `warnings` are surfaced to the editor but
/// never block anything.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ValidationReport {
    /// Problems that make the graph unrunnable or silently lossy.
    pub errors: Vec<BlueprintError>,
    /// Problems the engine tolerates but an author almost certainly wants to
    /// know about.
    pub warnings: Vec<BlueprintError>,
}

impl ValidationReport {
    /// Whether the blueprint may be stored and executed.
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Validates a blueprint, collecting every problem found.
///
/// Returning all of them at once matters for the visual editor: fixing one
/// wiring mistake at a time through a run is far slower than seeing the full
/// list up front.
///
/// An unconnected data input is a *warning*, not an error: executors read most
/// configuration from `node.data`, so a pin is an optional wiring point rather
/// than the only source of a value, and the `optional` flag on pins does not
/// reliably reflect which inputs an executor truly needs.
pub fn validate(blueprint: &Blueprint) -> ValidationReport {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    if blueprint.node(blueprint.entry_node_id).is_none() {
        errors.push(BlueprintError::MissingEntryNode(blueprint.entry_node_id));
    }

    // Data inputs that received at least one edge, so an unconnected check does
    // not re-report a pin that a malformed edge merely pretended to fill.
    let mut connected_inputs: HashSet<(NodeId, super::blueprint::PinId)> = HashSet::new();

    for edge in &blueprint.edges {
        if blueprint.node(edge.source_node).is_none() || blueprint.node(edge.target_node).is_none()
        {
            errors.push(BlueprintError::UnknownEdgeNode {
                edge: edge.clone(),
            });
            continue;
        }
        let (Some(source), Some(target)) =
            (blueprint.pin(edge.source_pin), blueprint.pin(edge.target_pin))
        else {
            errors.push(BlueprintError::UnknownPin {
                edge: edge.clone(),
            });
            continue;
        };
        // A pin id that resolves globally but sits on another node would
        // otherwise be silently wired to the wrong place.
        if source.id != edge.source_pin
            || !blueprint
                .node(edge.source_node)
                .is_some_and(|n| n.pins.iter().any(|p| p.id == edge.source_pin))
        {
            let owner = blueprint
                .nodes
                .iter()
                .find(|n| n.pins.iter().any(|p| p.id == edge.source_pin))
                .map(|n| n.id)
                .unwrap_or(edge.source_node);
            errors.push(BlueprintError::PinNotOnNode {
                edge: edge.clone(),
                owner,
            });
            continue;
        }
        if !blueprint
            .node(edge.target_node)
            .is_some_and(|n| n.pins.iter().any(|p| p.id == edge.target_pin))
        {
            let owner = blueprint
                .nodes
                .iter()
                .find(|n| n.pins.iter().any(|p| p.id == edge.target_pin))
                .map(|n| n.id)
                .unwrap_or(edge.target_node);
            errors.push(BlueprintError::PinNotOnNode {
                edge: edge.clone(),
                owner,
            });
            continue;
        }

        match (source.pin_type, target.pin_type) {
            (PinType::ExecOutput, PinType::ExecInput) => {}
            (PinType::DataOutput, PinType::DataInput) => {
                connected_inputs.insert((edge.target_node, edge.target_pin));
                if !compatible(&source.data_type, &target.data_type) {
                    errors.push(BlueprintError::IncompatibleTypes {
                        edge: edge.clone(),
                        source: source.data_type.clone(),
                        target: target.data_type.clone(),
                    });
                }
            }
            (PinType::ExecOutput, PinType::DataInput)
            | (PinType::DataOutput, PinType::ExecInput) => {
                errors.push(BlueprintError::PinKindMismatch {
                    edge: edge.clone(),
                    source_kind: source.pin_type,
                    target_kind: target.pin_type,
                });
            }
            // A source must be an *output* and a target must be an *input*;
            // anything else is a wiring mistake regardless of pin family.
            _ => errors.push(BlueprintError::ExecPinDirection {
                edge: edge.clone(),
            }),
        }
    }

    for node in &blueprint.nodes {
        for pin in node.pins.iter().filter(|p| p.pin_type == PinType::DataInput) {
            let filled = connected_inputs.contains(&(node.id, pin.id))
                || pin.default.is_some()
                || crate::node_catalog::inline_value(node, pin).is_some();
            if !filled && !pin.optional {
                warnings.push(BlueprintError::UnconnectedRequiredInput {
                    node_id: node.id,
                    kind: node.kind.clone(),
                    pin: pin.name.clone(),
                });
            }
        }
    }

    if has_exec_cycle(blueprint) {
        errors.push(BlueprintError::ExecCycle);
    }

    ValidationReport {
        errors,
        warnings,
    }
}

/// Validates existing instance pins using the registry's canonical types and
/// metadata. Ids, edges and dynamic function/tool pins remain instance-owned.
/// Missing inputs remain warnings, including for legacy data-only nodes.
pub fn validate_with_catalog(
    blueprint: &Blueprint,
    catalog: &crate::node_catalog::NodeCatalog,
) -> ValidationReport {
    let mut resolved = blueprint.clone();
    for node in &mut resolved.nodes {
        let Some(signature) = catalog.resolve(&node.kind, &node.data) else {
            continue;
        };
        for pin in &mut node.pins {
            if let Some(spec) = signature.pin(pin) {
                pin.data_type = spec.data_type.clone();
                if pin.default.is_none() {
                    pin.default = spec.default.clone();
                }
                pin.optional = spec.optional;
            }
        }
    }
    let mut report = validate(&resolved);
    for node in &blueprint.nodes {
        let function_binding = node.data.get("function").and_then(|v|v.as_str()).and_then(|name|catalog.addon_function_bindings.get(name));
        if node.kind=="CallFunction" {
            let key=crate::node_catalog::addon::FUNCTION_BINDING_KEY;
            let result=match function_binding {
                Some(binding)=>catalog.resolve(&node.kind,&node.data).ok_or_else(||"Addon function unavailable".into()).and_then(|s|crate::node_catalog::addon::validate_bound_instance(node,&s,binding,key)),
                None if node.data.get(key).is_some()=>Err("Addon function or dependency is missing or disabled".into()),
                _=>Ok(())
            };
            if let Err(detail)=result {report.errors.push(BlueprintError::AddonContract{node_id:node.id,detail});}
        }
        let result = match catalog.addon_bindings.get(&node.kind) {
            Some(binding) => catalog.get(&node.kind).ok_or_else(|| "Addon node unavailable".into())
                .and_then(|s| crate::node_catalog::addon::validate_instance(node,s,binding)),
            None if node.data.get(crate::node_catalog::addon::BINDING_KEY).is_some() => Err("Addon package is missing, disabled or unavailable".into()),
            None => Ok(()),
        };
        if let Err(detail)=result { report.errors.push(BlueprintError::AddonContract {node_id:node.id,detail}); }
    }
    report
}

/// Visit state for the execution-graph cycle check.
#[derive(Clone, Copy, PartialEq)]
enum Mark {
    /// On the current DFS path: reaching it again closes a cycle.
    Open,
    /// Fully explored; reaching it again is not a cycle.
    Done,
}

/// Whether the execution graph is acyclic.
///
/// Only execution edges can form a cycle: data edges merely gate when a node
/// becomes runnable, and a data cycle is how a feedback node is expressed.
pub fn has_exec_cycle(blueprint: &Blueprint) -> bool {
    let mut state: std::collections::HashMap<NodeId, Mark> = std::collections::HashMap::new();
    for node in &blueprint.nodes {
        if visit_exec(node.id, blueprint, &mut state) {
            return true;
        }
    }
    false
}

fn visit_exec(
    node_id: NodeId,
    blueprint: &Blueprint,
    state: &mut std::collections::HashMap<NodeId, Mark>,
) -> bool {
    match state.get(&node_id) {
        Some(Mark::Done) => return false,
        Some(Mark::Open) => return true,
        None => {}
    }
    state.insert(node_id, Mark::Open);
    for edge in blueprint.outgoing_edges(node_id) {
        // Only an edge leaving an execution output can continue the flow.
        if blueprint.pin(edge.source_pin).is_some_and(|p| p.pin_type == PinType::ExecOutput)
            && visit_exec(edge.target_node, blueprint, state)
        {
            return true;
        }
    }
    state.insert(node_id, Mark::Done);
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::blueprint::{Node, NodeType, Pin};
    use uuid::Uuid;

    fn pin(kind: PinType, ty: DataType) -> Pin {
        Pin {
            id: Uuid::new_v4(),
            key: None,
            name: "p".to_string(),
            pin_type: kind,
            data_type: ty,
            default: None,
            optional: false,
            choices: Vec::new(),
            description: None,
        }
    }

    fn node(kind: &str, pins: Vec<Pin>) -> Node {
        Node {
            id: Uuid::new_v4(),
            node_type: NodeType::Function,
            kind: kind.to_string(),
            position: (0.0, 0.0),
            pins,
            data: serde_json::Value::Null,
        }
    }

    /// Wires `source`'s first pin to `target`'s first pin.
    fn connect(source: &Node, target: &Node) -> Edge {
        Edge {
            id: Uuid::new_v4(),
            source_node: source.id,
            source_pin: source.pins[0].id,
            target_node: target.id,
            target_pin: target.pins[0].id,
        }
    }

    fn blueprint(nodes: Vec<Node>, edges: Vec<Edge>) -> Blueprint {
        Blueprint {
            id: Uuid::new_v4(),
            name: "test".into(),
            entry_node_id: nodes[0].id,
            nodes,
            edges,
        }
    }

    #[test]
    fn accepts_a_well_formed_graph() {
        let producer = node("A", vec![pin(PinType::DataOutput, DataType::Int)]);
        let consumer = node("B", vec![pin(PinType::DataInput, DataType::Float)]);
        // Int -> Float is a lossless widening and stays legal.
        let bp = blueprint(
            vec![producer.clone(), consumer.clone()],
            vec![connect(&producer, &consumer)],
        );
        let report = validate(&bp);
        assert!(report.is_ok(), "{:?}", report);
    }

    #[test]
    fn accepts_scalar_into_string() {
        // A scalar has a faithful textual form, and executors render one when a
        // string pin receives a scalar, so this is not a lossy narrowing.
        for scalar in [DataType::Int, DataType::Float, DataType::Bool] {
            let scalar_name = format!("{scalar:?}");
            let producer = node("A", vec![pin(PinType::DataOutput, scalar)]);
            let consumer = node("B", vec![pin(PinType::DataInput, DataType::String)]);
            let bp = blueprint(
                vec![producer.clone(), consumer.clone()],
                vec![connect(&producer, &consumer)],
            );
            let report = validate(&bp);
            assert!(report.is_ok(), "{scalar_name}: {report:?}");
        }
    }

    #[test]
    fn rejects_float_into_int() {
        let producer = node("A", vec![pin(PinType::DataOutput, DataType::Float)]);
        let consumer = node("B", vec![pin(PinType::DataInput, DataType::Int)]);
        let bp = blueprint(
            vec![producer.clone(), consumer.clone()],
            vec![connect(&producer, &consumer)],
        );
        let report = validate(&bp);
        assert!(
            report.errors.iter().any(|e| matches!(e, BlueprintError::IncompatibleTypes { .. })),
            "expected a type error, got {report:?}"
        );
    }

    #[test]
    fn rejects_exec_output_wired_to_data_input() {
        let producer = node("A", vec![pin(PinType::ExecOutput, DataType::Void)]);
        let consumer = node("B", vec![pin(PinType::DataInput, DataType::Int)]);
        let bp = blueprint(
            vec![producer.clone(), consumer.clone()],
            vec![connect(&producer, &consumer)],
        );
        assert!(
            validate(&bp)
                .errors
                .iter()
                .any(|e| matches!(e, BlueprintError::PinKindMismatch { .. })),
        );
    }

    #[test]
    fn rejects_data_output_wired_to_exec_input() {
        let producer = node("A", vec![pin(PinType::DataOutput, DataType::Int)]);
        let consumer = node("B", vec![pin(PinType::ExecInput, DataType::Void)]);
        let bp = blueprint(
            vec![producer.clone(), consumer.clone()],
            vec![connect(&producer, &consumer)],
        );
        assert!(
            validate(&bp)
                .errors
                .iter()
                .any(|e| matches!(e, BlueprintError::PinKindMismatch { .. })),
        );
    }

    #[test]
    fn reports_unconnected_required_input() {
        let only = node("A", vec![pin(PinType::DataInput, DataType::Int)]);
        let bp = blueprint(vec![only], vec![]);
        // Tolerated at run time, but an author still wants to see it.
        assert!(validate(&bp).warnings.len() == 1, "{:?}", validate(&bp));
        assert!(validate(&bp).is_ok());
    }

    #[test]
    fn optional_input_may_stay_unconnected() {
        let mut optional = pin(PinType::DataInput, DataType::Int);
        optional.optional = true;
        let bp = blueprint(vec![node("A", vec![optional])], vec![]);
        let report = validate(&bp);
        assert!(report.is_ok(), "{:?}", report);
    }

    #[test]
    fn reports_edge_to_unknown_pin() {
        let producer = node("A", vec![pin(PinType::DataOutput, DataType::Int)]);
        let consumer = node("B", vec![pin(PinType::DataInput, DataType::Int)]);
        let mut bad = connect(&producer, &consumer);
        bad.target_pin = Uuid::new_v4();
        let bp = blueprint(vec![producer, consumer], vec![bad]);
        assert!(
            validate(&bp).errors.iter().any(|e| matches!(e, BlueprintError::UnknownPin { .. })),
        );
    }

    #[test]
    fn reports_edge_to_unknown_node() {
        let producer = node("A", vec![pin(PinType::DataOutput, DataType::Int)]);
        let consumer = node("B", vec![pin(PinType::DataInput, DataType::Int)]);
        let mut bad = connect(&producer, &consumer);
        bad.target_node = Uuid::new_v4();
        let bp = blueprint(vec![producer, consumer], vec![bad]);
        assert!(
            validate(&bp)
                .errors
                .iter()
                .any(|e| matches!(e, BlueprintError::UnknownEdgeNode { .. })),
        );
    }

    #[test]
    fn detects_exec_cycle() {
        let a = node("A", vec![pin(PinType::ExecOutput, DataType::Void)]);
        let b = node("B", vec![pin(PinType::ExecOutput, DataType::Void)]);
        let bp = blueprint(vec![a.clone(), b.clone()], vec![connect(&a, &b), connect(&b, &a)]);
        assert!(validate(&bp).errors.contains(&BlueprintError::ExecCycle));
    }

    #[test]
    fn data_cycle_is_not_an_exec_cycle() {
        // A feedback node legitimately points back at an earlier data input.
        let a = node("A", vec![pin(PinType::DataOutput, DataType::Int)]);
        let b = node("B", vec![pin(PinType::DataInput, DataType::Int)]);
        let bp = blueprint(vec![a.clone(), b.clone()], vec![connect(&a, &b), connect(&b, &a)]);
        assert!(!validate(&bp).errors.contains(&BlueprintError::ExecCycle));
    }
}
