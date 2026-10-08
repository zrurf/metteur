//! Built-in node executors.

pub mod abstract_node;
pub mod arithmetic;
pub mod call_llm;
pub mod collections;
pub mod context_nodes;
pub mod control;
pub mod flow_nodes;
pub mod foreach;
pub mod function;
pub mod judge;
pub mod lsp_nodes;
pub mod pure;
pub mod string_ops;
pub mod tool;
pub mod validator;
pub mod variables;

pub use abstract_node::AbstractExecutor;
pub use arithmetic::{AddExecutor, DivideExecutor, MultiplyExecutor, SubtractExecutor};
pub use call_llm::CallLlmExecutor;
pub use collections::{
    JsonGetExecutor, JsonSetExecutor, ListAppendExecutor, ListContainsExecutor, ListCreateExecutor,
    ListGetExecutor, ListLengthExecutor,
};
pub use context_nodes::{
    ContextCloneExecutor, ContextCreateExecutor, ContextFilterExecutor, ContextMergeExecutor,
    ContextReleaseExecutor, ContextToTextExecutor, ContextTrimExecutor,
};
pub use control::{BranchExecutor, SwitchExecutor};
pub use flow_nodes::{DelayExecutor, OversightCheckpointExecutor, RequestApprovalExecutor};
pub use foreach::ForEachExecutor;
pub use function::{CallFunctionExecutor, FunctionEntryExecutor, FunctionExitExecutor};
pub use judge::JudgeExecutor;
pub use lsp_nodes::LspCheckExecutor;
pub use pure::{
    AbsExecutor, AndExecutor, EqualExecutor, GreaterEqualExecutor, GreaterExecutor,
    LessEqualExecutor, LessExecutor, MaxExecutor, MinExecutor, ModuloExecutor, NotEqualExecutor,
    NotExecutor, OrExecutor, PowerExecutor, RoundExecutor, XorExecutor,
};
pub use string_ops::{
    ConcatExecutor, ContainsExecutor, LengthExecutor, LowerExecutor, ParseJsonExecutor,
    ReplaceExecutor, SubstringExecutor, ToBoolExecutor, ToFloatExecutor, ToIntExecutor,
    ToJsonExecutor, ToStringExecutor, TrimExecutor, UpperExecutor,
};
pub use tool::ToolExecutor;
pub use validator::ValidatorExecutor;
pub use variables::{VariableGetExecutor, VariableSetExecutor};

/// A terminal event node. It produces no outputs; execution ends when no exec
/// edge leaves it.
pub struct EndExecutor;

#[async_trait::async_trait]
impl crate::registry::NodeExecutor for EndExecutor {
    fn kind(&self) -> &str {
        "End"
    }

    async fn execute(
        &self,
        _node: &Node,
        _inputs: &HashMap<PinId, Value>,
        _ctx: &mut crate::execution::context::ExecutionContext,
    ) -> DaemonResult<HashMap<PinId, Value>> {
        Ok(HashMap::new())
    }
}

use std::collections::HashMap;

use metteur_shared::{Node, PinId, PinType, Value};

use crate::error::{DaemonError, DaemonResult};

/// The Start event node. Produces data output values from its `data` object,
/// plus an initial context manager on its `Context` output pin when present.
pub struct StartExecutor;

#[async_trait::async_trait]
impl crate::registry::NodeExecutor for StartExecutor {
    fn kind(&self) -> &str {
        "Start"
    }

    async fn execute(
        &self,
        node: &Node,
        _inputs: &HashMap<PinId, Value>,
        ctx: &mut crate::execution::context::ExecutionContext,
    ) -> DaemonResult<HashMap<PinId, Value>> {
        let mut outputs = HashMap::new();
        if let serde_json::Value::Object(map) = &node.data {
            for pin in &node.pins {
                if pin.pin_type != PinType::DataOutput {
                    continue;
                }
                if let Some(value) = map.get(&pin.name) {
                    outputs.insert(pin.id, json_to_value(value));
                }
            }
        }
        // The initial context manager carries the environment's system
        // fragments so downstream nodes start from a known baseline.
        if let Some(pin) =
            node.pins.iter().find(|p| p.name == "Context" && p.pin_type == PinType::DataOutput)
        {
            let mut system_fragments = crate::harness::fragments(ctx).await;
            system_fragments.extend(ctx.addon_fragments.iter().cloned());
            let context = metteur_shared::llm::ContextManager {
                system_fragments,
                ..Default::default()
            };
            outputs.insert(pin.id, Value::Context(context));
        }
        Ok(outputs)
    }
}

/// Converts a JSON value into a shared [`Value`].
pub(crate) fn json_to_value(json: &serde_json::Value) -> Value {
    match json {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else {
                Value::Float(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => Value::String(s.clone()),
        serde_json::Value::Array(items) => Value::List(items.iter().map(json_to_value).collect()),
        serde_json::Value::Object(_) => Value::Json(json.clone()),
    }
}

/// Reads a numeric input by pin name.
pub(crate) fn numeric_input(
    inputs: &HashMap<PinId, Value>,
    node: &Node,
    name: &str,
) -> DaemonResult<f64> {
    let pin = node
        .pins
        .iter()
        .find(|p| p.name == name)
        .ok_or_else(|| DaemonError::Execution(format!("missing pin {name}")))?;
    let value = inputs
        .get(&pin.id)
        .ok_or_else(|| DaemonError::Execution(format!("missing input {name}")))?;
    value.as_float().ok_or_else(|| DaemonError::Execution(format!("input {name} is not numeric")))
}

/// Writes a numeric value to the named output pin.
pub(crate) fn numeric_output(
    node: &Node,
    name: &str,
    value: f64,
) -> DaemonResult<HashMap<PinId, Value>> {
    let pin = node
        .pins
        .iter()
        .find(|p| p.name == name)
        .ok_or_else(|| DaemonError::Execution(format!("missing pin {name}")))?;
    Ok(HashMap::from([(pin.id, Value::Float(value))]))
}

/// Writes an integer value to the named output pin.
pub(crate) fn int_output(
    node: &Node,
    name: &str,
    value: i64,
) -> DaemonResult<HashMap<PinId, Value>> {
    let pin = node
        .pins
        .iter()
        .find(|p| p.name == name)
        .ok_or_else(|| DaemonError::Execution(format!("missing pin {name}")))?;
    Ok(HashMap::from([(pin.id, Value::Int(value))]))
}

/// Writes a boolean value to the named output pin.
pub(crate) fn bool_output(
    node: &Node,
    name: &str,
    value: bool,
) -> DaemonResult<HashMap<PinId, Value>> {
    let pin = node
        .pins
        .iter()
        .find(|p| p.name == name)
        .ok_or_else(|| DaemonError::Execution(format!("missing pin {name}")))?;
    Ok(HashMap::from([(pin.id, Value::Bool(value))]))
}

/// Writes a string value to the named output pin.
pub(crate) fn string_output(
    node: &Node,
    name: &str,
    value: impl Into<String>,
) -> DaemonResult<HashMap<PinId, Value>> {
    let pin = node
        .pins
        .iter()
        .find(|p| p.name == name)
        .ok_or_else(|| DaemonError::Execution(format!("missing pin {name}")))?;
    Ok(HashMap::from([(pin.id, Value::String(value.into()))]))
}

/// Reads a data input by pin name.
pub(crate) fn value_input<'a>(
    node: &Node,
    inputs: &'a HashMap<PinId, Value>,
    name: &str,
) -> DaemonResult<&'a Value> {
    let pin = node
        .pins
        .iter()
        .find(|p| p.name == name && p.pin_type == PinType::DataInput)
        .ok_or_else(|| DaemonError::Execution(format!("missing pin {name}")))?;
    inputs.get(&pin.id).ok_or_else(|| DaemonError::Execution(format!("missing input {name}")))
}

/// Reads a data input, falling back to the node's `data` constant of the same
/// name, its pin key, or its pin id (DSL literals and frontend inline values
/// land in one of these three slots).
pub(crate) fn input_or_data(
    node: &Node,
    inputs: &HashMap<PinId, Value>,
    name: &str,
) -> DaemonResult<Value> {
    if let Ok(value) = value_input(node, inputs, name) {
        return Ok(value.clone());
    }
    let find_pin = || node.pins.iter().find(|p| p.name == name && p.pin_type == PinType::DataInput);
    let data = node
        .data
        .get(name)
        .or_else(|| find_pin().and_then(|p| p.key.as_deref().and_then(|k| node.data.get(k))))
        .or_else(|| {
            // The web UI keys inline values by pin id as a last-resort slot.
            find_pin().and_then(|p| node.data.get(p.id.to_string().as_str()))
        })
        .map(json_to_value);
    data.ok_or_else(|| DaemonError::Execution(format!("missing input {name}")))
}

/// Reads a boolean input by pin name.
pub(crate) fn bool_input(
    node: &Node,
    inputs: &HashMap<PinId, Value>,
    name: &str,
) -> DaemonResult<bool> {
    let value = input_or_data(node, inputs, name)?;
    value.as_bool().ok_or_else(|| DaemonError::Execution(format!("input {name} is not boolean")))
}

/// Reads an integer input, tolerating float values produced by math nodes.
pub(crate) fn int_input(
    node: &Node,
    inputs: &HashMap<PinId, Value>,
    name: &str,
) -> DaemonResult<i64> {
    let value = input_or_data(node, inputs, name)?;
    match value {
        Value::Int(i) => Ok(i),
        Value::Float(f) => Ok(f as i64),
        Value::Json(j) => j
            .as_i64()
            .or_else(|| j.as_f64().map(|f| f as i64))
            .ok_or_else(|| DaemonError::Execution(format!("input {name} is not an integer"))),
        _ => Err(DaemonError::Execution(format!("input {name} is not an integer"))),
    }
}

/// Reads a string input by pin name, coercing scalars when possible.
pub(crate) fn string_input(
    node: &Node,
    inputs: &HashMap<PinId, Value>,
    name: &str,
) -> DaemonResult<String> {
    let value = input_or_data(node, inputs, name)?;
    match value {
        Value::String(s) => Ok(s),
        Value::Int(i) => Ok(i.to_string()),
        Value::Float(f) => Ok(f.to_string()),
        Value::Bool(b) => Ok(b.to_string()),
        Value::Json(j) => Ok(j.to_string()),
        _ => Err(DaemonError::Execution(format!("input {name} is not text"))),
    }
}

/// Reads a list input by pin name.
pub(crate) fn list_input(
    node: &Node,
    inputs: &HashMap<PinId, Value>,
    name: &str,
) -> DaemonResult<Vec<Value>> {
    let value = input_or_data(node, inputs, name)?;
    match value {
        Value::List(items) => Ok(items),
        Value::Json(j) => {
            let items = j.as_array().map(|a| a.iter().map(json_to_value).collect::<Vec<_>>());
            items.ok_or_else(|| DaemonError::Execution(format!("input {name} is not a list")))
        }
        _ => Err(DaemonError::Execution(format!("input {name} is not a list"))),
    }
}

/// Converts a value into a textual representation.
pub(crate) fn value_to_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Json(j) => j.to_string(),
        // Scalars render as their value, not as their Rust debug form: a tool
        // result of `true` must read "true", not "Bool(true)", both to the model
        // and in the transcript.
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        other => format!("{other:?}"),
    }
}

/// Converts a value into its JSON representation.
pub(crate) fn value_to_json(value: &Value) -> serde_json::Value {
    match value {
        Value::Null | Value::Context(_) => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Int(i) => serde_json::Value::Number((*i).into()),
        Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Value::String(s) => serde_json::Value::String(s.clone()),
        Value::List(l) => serde_json::Value::Array(l.iter().map(value_to_json).collect()),
        Value::Json(j) => j.clone(),
    }
}

/// Returns true when a value carries no meaningful content.
pub(crate) fn is_empty(value: &Value) -> bool {
    match value {
        Value::Null | Value::Context(_) => true,
        Value::Bool(b) => !*b,
        Value::Int(i) => *i == 0,
        Value::Float(f) => *f == 0.0,
        Value::String(s) => s.is_empty(),
        Value::List(l) => l.is_empty(),
        Value::Json(j) => match j {
            serde_json::Value::Null => true,
            serde_json::Value::Bool(b) => !*b,
            serde_json::Value::Number(n) => n.as_f64().map(|f| f == 0.0).unwrap_or(false),
            serde_json::Value::String(s) => s.is_empty(),
            serde_json::Value::Array(a) => a.is_empty(),
            serde_json::Value::Object(o) => o.is_empty(),
        },
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use metteur_shared::{DataType, NodeType, Pin};
    use std::sync::Arc;

    /// Builds a pure node with the given data inputs (as `Float`) and a
    /// `Result` output, wired with fresh pin ids.
    fn node_with_float_inputs(kind: &str, names: &[&str]) -> Node {
        let mut pins = vec![
            Pin::exec(PinType::ExecInput, uuid::Uuid::new_v4()),
            Pin::exec(PinType::ExecOutput, uuid::Uuid::new_v4()),
        ];
        for name in names {
            pins.push(Pin::data(*name, PinType::DataInput, DataType::Float, uuid::Uuid::new_v4()));
        }
        pins.push(Pin::data("Result", PinType::DataOutput, DataType::Any, uuid::Uuid::new_v4()));
        Node {
            id: uuid::Uuid::new_v4(),
            node_type: NodeType::Pure,
            kind: kind.to_string(),
            position: (0.0, 0.0),
            pins,
            data: serde_json::Value::Null,
        }
    }

    /// Runs `kind` with float inputs and returns outputs by pin name.
    pub(crate) async fn exec_floats(
        kind: &str,
        inputs: &[(&str, f64)],
    ) -> DaemonResult<HashMap<String, Value>> {
        let node_registry = crate::registry::NodeRegistry::with_builtins();
        let node =
            node_with_float_inputs(kind, &inputs.iter().map(|(n, _)| *n).collect::<Vec<_>>());
        let mut values = HashMap::new();
        for (name, value) in inputs {
            let pin = node.pins.iter().find(|p| p.name == *name).unwrap();
            values.insert(pin.id, Value::Float(*value));
        }
        let executor = node_registry
            .get(kind)
            .ok_or_else(|| crate::error::DaemonError::Execution(format!("no node {kind}")))?;
        let mut ctx = crate::execution::context::ExecutionContext::new(
            Arc::new(crate::registry::Registry::with_builtins()),
            crate::llm::LlmClientFactory::new(),
            std::env::temp_dir(),
        );
        let outputs = executor.execute(&node, &values, &mut ctx).await?;
        Ok(outputs
            .into_iter()
            .map(|(id, value)| {
                let name = node
                    .pins
                    .iter()
                    .find(|p| p.id == id)
                    .map(|p| p.name.clone())
                    .unwrap_or_default();
                (name, value)
            })
            .collect())
    }

    /// Runs `kind` with boolean inputs and returns outputs by pin name.
    pub(crate) async fn exec_bools(
        kind: &str,
        inputs: &[(&str, bool)],
    ) -> DaemonResult<HashMap<String, Value>> {
        let node_registry = crate::registry::NodeRegistry::with_builtins();
        let node =
            node_with_float_inputs(kind, &inputs.iter().map(|(n, _)| *n).collect::<Vec<_>>());
        let mut values = HashMap::new();
        for (name, value) in inputs {
            let pin = node.pins.iter().find(|p| p.name == *name).unwrap();
            values.insert(pin.id, Value::Bool(*value));
        }
        let executor = node_registry
            .get(kind)
            .ok_or_else(|| crate::error::DaemonError::Execution(format!("no node {kind}")))?;
        let mut ctx = crate::execution::context::ExecutionContext::new(
            Arc::new(crate::registry::Registry::with_builtins()),
            crate::llm::LlmClientFactory::new(),
            std::env::temp_dir(),
        );
        let outputs = executor.execute(&node, &values, &mut ctx).await?;
        Ok(outputs
            .into_iter()
            .map(|(id, value)| {
                let name = node
                    .pins
                    .iter()
                    .find(|p| p.id == id)
                    .map(|p| p.name.clone())
                    .unwrap_or_default();
                (name, value)
            })
            .collect())
    }
}
