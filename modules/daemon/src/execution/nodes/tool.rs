//! Tool node executor.

use std::collections::HashMap;

use async_trait::async_trait;
use metteur_shared::{Node, PinId, Value};

use crate::error::{DaemonError, DaemonResult};
use crate::execution::context::ExecutionContext;
use crate::registry::NodeExecutor;

use super::string_output;

/// A node that invokes a registered tool.
///
/// The tool name is read from the node's `data.tool_name`. Data input pin
/// values are passed to the tool as a single JSON object keyed by pin name,
/// so tools can resolve arguments by name.
pub struct ToolExecutor;

#[async_trait]
impl NodeExecutor for ToolExecutor {
    fn kind(&self) -> &str {
        "Tool"
    }

    async fn execute(
        &self,
        node: &Node,
        inputs: &HashMap<PinId, Value>,
        ctx: &mut ExecutionContext,
    ) -> DaemonResult<HashMap<PinId, Value>> {
        let connected_name = node
            .pins
            .iter()
            .find(|p| p.name == "ToolName" && p.pin_type == metteur_shared::PinType::DataInput)
            .and_then(|pin| inputs.get(&pin.id))
            .and_then(Value::as_str);
        let tool_name = connected_name
            .or_else(|| node.data.get("tool_name").and_then(|v| v.as_str()))
            .ok_or_else(|| DaemonError::Execution("tool node missing tool_name".to_string()))?;

        if !crate::registry::capabilities::allowed(ctx,tool_name) {
            return Err(DaemonError::PermissionDenied("Tool is not allowed for this host role".into()));
        }
        let tool = ctx
            .registry
            .tool(tool_name)
            .ok_or_else(|| DaemonError::NotFound(format!("tool {tool_name}")))?;

        let mut args_obj = serde_json::Map::new();
        for pin in node.pins.iter().filter(|p| p.pin_type == metteur_shared::PinType::DataInput) {
            if pin.name == "ToolName" {
                continue;
            }
            // Fall back to the node's inline constant for the pin when nothing
            // is wired: DSL literals and canvas-authored values live in
            // `node.data`, and a tool argument supplied that way must reach
            // the tool just like a connected edge would.
            let value = match inputs.get(&pin.id) {
                Some(value) => value.clone(),
                None => super::input_or_data(node, inputs, &pin.name).unwrap_or(Value::Null),
            };
            // Absent optional arguments must be omitted so the tool's own
            // defaults apply; explicit null arguments remain representable.
            if pin.optional
                && !inputs.contains_key(&pin.id)
                && metteur_shared::node_catalog::inline_value(node, pin).is_none()
            {
                continue;
            }
            args_obj
                .insert(pin.key.clone().unwrap_or_else(|| pin.name.clone()), value_to_json(&value));
        }
        let args = vec![Value::Json(serde_json::Value::Object(args_obj.clone()))];

        ctx.transaction_log.record_tool_call(tool_name.to_string(), args.clone());
        ctx.audit("tool.call", serde_json::json!({ "name": tool_name, "args": args_obj }));
        if let Some(metrics) = &ctx.metrics {
            metrics.tool_calls_total.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        let result = tool.call(&args, ctx).await?;
        let text = value_to_text(&result);
        string_output(node, "Result", text)
    }
}

/// Converts a shared value into a JSON value.
fn value_to_json(value: &Value) -> serde_json::Value {
    match value {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Int(i) => serde_json::Value::Number((*i).into()),
        Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Value::String(s) => serde_json::Value::String(s.clone()),
        Value::List(items) => serde_json::Value::Array(items.iter().map(value_to_json).collect()),
        Value::Json(j) => j.clone(),
        Value::Context(_) => serde_json::Value::Null,
    }
}

/// Converts a tool result value into a textual representation.
fn value_to_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Json(j) => j.to_string(),
        other => format!("{other:?}"),
    }
}
