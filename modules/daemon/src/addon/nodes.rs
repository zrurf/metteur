//! Package executors share the existing permission/journal-aware Wasm adapter.
use super::{
    AddonTool,
    manifest::{NodeEntry, ToolEntry},
    package::Package,
    runtime::Permission,
};
use crate::{
    DaemonError, DaemonResult, execution::context::ExecutionContext, registry::NodeExecutor,
};
use metteur_shared::{
    Node, PinId, PinType, Value,
    node_catalog::{
        NodeSignature,
        addon::{json_matches, validate_instance},
    },
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

pub(super) struct AddonNode {
    tool: AddonTool,
    signature: NodeSignature,
    binding: serde_json::Value,
}
impl AddonNode {
    pub(super) fn new(
        package: Arc<Package>,
        entry: &NodeEntry,
        mut permissions: HashSet<Permission>,
        timeout: u64,
    ) -> Self {
        let mut signature = entry.signature.clone();
        signature.kind = format!("{}{}", super::pascal(&package.identity.id), entry.name);
        signature.executor_kind = signature.kind.clone();
        if signature.node_type == metteur_shared::NodeType::Pure {
            permissions.clear();
        }
        let binding = serde_json::to_value(&package.identity).expect("serializable identity");
        let tool = AddonTool::new(
            package,
            &ToolEntry {
                name: entry.name.clone(),
                function: entry.function.clone(),
                description: signature.description.clone(),
                parameters: Default::default(),
            },
            permissions,
            timeout,
        );
        Self {
            tool,
            signature,
            binding,
        }
    }
}
fn failure(message: &str) -> DaemonError {
    DaemonError::Addon(message.into())
}
pub(crate) fn raw(value: &Value) -> DaemonResult<serde_json::Value> {
    Ok(match value {
        Value::Null => serde_json::Value::Null,
        Value::Bool(v) => serde_json::json!(v),
        Value::Int(v) => serde_json::json!(v),
        Value::Float(v) if v.is_finite() => serde_json::json!(v),
        Value::String(v) => serde_json::json!(v),
        Value::Json(v) => v.clone(),
        Value::List(v) => serde_json::Value::Array(v.iter().map(raw).collect::<DaemonResult<_>>()?),
        _ => return Err(failure("Addon nodes accept only bounded JSON values")),
    })
}
#[async_trait::async_trait]
impl NodeExecutor for AddonNode {
    fn kind(&self) -> &str {
        &self.signature.kind
    }
    fn signature(&self) -> NodeSignature {
        self.signature.clone()
    }
    async fn execute(
        &self,
        node: &Node,
        inputs: &HashMap<PinId, Value>,
        ctx: &mut ExecutionContext,
    ) -> DaemonResult<HashMap<PinId, Value>> {
        validate_instance(node, &self.signature, &self.binding).map_err(DaemonError::Addon)?;
        let mut args = serde_json::Map::new();
        for pin in node.pins.iter().filter(|p| p.pin_type == PinType::DataInput) {
            let value = inputs
                .get(&pin.id)
                .map(raw)
                .transpose()?
                .or_else(|| metteur_shared::node_catalog::inline_value(node, pin).cloned())
                .or_else(|| pin.default.clone());
            let value = match value {
                Some(v) => v,
                None if pin.optional => serde_json::Value::Null,
                None => return Err(failure("Addon node required input is missing")),
            };
            if !(value.is_null() && pin.optional)
                && (!json_matches(&value, &pin.data_type)
                    || (!pin.choices.is_empty()
                        && !pin.choices.iter().any(|c| Some(c.as_str()) == value.as_str())))
            {
                return Err(failure("Addon node input violates its declared type or choices"));
            }
            args.insert(pin.key.clone().expect("validated key"), value);
        }
        if inputs
            .keys()
            .any(|id| !node.pins.iter().any(|p| p.id == *id && p.pin_type == PinType::DataInput))
        {
            return Err(failure("Addon node received an unknown input pin"));
        }
        let result =
            self.tool.invoke_json(serde_json::json!({"inputs":args}).to_string(), ctx).await?;
        let Value::Json(result) = result else {
            return Err(failure("Addon node output is not JSON"));
        };
        let object = result
            .as_object()
            .filter(|o| o.len() == 1)
            .and_then(|o| o.get("outputs"))
            .and_then(|v| v.as_object())
            .ok_or_else(|| failure("Addon node must return only an outputs object"))?;
        let pins: Vec<_> = node.pins.iter().filter(|p| p.pin_type == PinType::DataOutput).collect();
        if object.len() != pins.len() {
            return Err(failure("Addon node output pin set differs from its signature"));
        }
        let mut output = HashMap::new();
        for pin in pins {
            let value = object
                .get(pin.key.as_ref().expect("validated key"))
                .ok_or_else(|| failure("Addon node output pin is missing"))?;
            if !json_matches(value, &pin.data_type)
                || (!pin.choices.is_empty()
                    && !pin.choices.iter().any(|c| Some(c.as_str()) == value.as_str()))
            {
                return Err(failure("Addon node output violates its declared type or choices"));
            }
            output.insert(pin.id, metteur_shared::model::value::json_to_value(value));
        }
        Ok(output)
    }
}

#[cfg(test)]
#[path = "nodes_tests.rs"]
pub(super) mod tests;
