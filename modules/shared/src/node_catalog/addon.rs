//! Fixed, version-bound contracts for package nodes.
use super::*;
use crate::{Node, NodeType};
use serde_json::Value;

pub const BINDING_KEY: &str = "_addon_binding";
pub const FUNCTION_BINDING_KEY: &str = "_addon_function_binding";

pub fn json_matches(value: &Value, ty: &DataType) -> bool {
    match ty {
        DataType::Any | DataType::Json => true,
        DataType::Void => value.is_null(),
        DataType::Bool => value.is_boolean(),
        DataType::Int => value.as_i64().is_some(),
        DataType::Float => value.as_f64().is_some_and(f64::is_finite),
        DataType::String | DataType::Choice => value.is_string(),
        DataType::List(inner) => {
            value.as_array().is_some_and(|a| a.iter().all(|v| json_matches(v, inner)))
        }
        DataType::Object(fields) => value.as_object().is_some_and(|o| {
            fields.iter().all(|(k, t)| o.get(k).is_some_and(|v| json_matches(v, t)))
        }),
        DataType::Context => false,
    }
}
fn portable_type(ty: &DataType, depth: usize) -> bool {
    depth < 8
        && match ty {
            DataType::Context | DataType::Void => false,
            DataType::List(t) => portable_type(t, depth + 1),
            DataType::Object(fields) => {
                fields.len() <= 64
                    && fields.iter().all(|(k, t)| identifier(k) && portable_type(t, depth + 1))
            }
            _ => true,
        }
}
fn identifier(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 128
        && text.as_bytes()[0].is_ascii_alphabetic()
        && text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
pub fn validate_signature(signature: &NodeSignature) -> Result<(), String> {
    let invalid = || "Invalid fixed addon node signature".to_string();
    if signature.dynamic_pins
        || signature.pins.len() > 64
        || serde_json::to_vec(signature).map_or(true, |bytes| bytes.len() > 16 * 1024)
        || signature.description.len() > 4096
        || !matches!(signature.node_type, NodeType::Function | NodeType::Pure)
    {
        return Err(invalid());
    }
    let mut keys = std::collections::HashSet::new();
    let mut names = std::collections::HashSet::new();
    let mut exec_in = 0;
    let mut exec_out = 0;
    for p in &signature.pins {
        if !identifier(&p.key)
            || !identifier(&p.name)
            || !keys.insert(&p.key)
            || !names.insert(&p.name)
            || p.description.as_ref().is_some_and(|d| d.len() > 4096)
            || p.choices.len() > 128
        {
            return Err(invalid());
        }
        match p.pin_type {
            PinType::ExecInput | PinType::ExecOutput => {
                if p.data_type != DataType::Void
                    || p.default.is_some()
                    || p.optional
                    || !p.choices.is_empty()
                {
                    return Err(invalid());
                }
                if p.pin_type == PinType::ExecInput {
                    exec_in += 1;
                } else {
                    exec_out += 1;
                }
            }
            _ => {
                if !portable_type(&p.data_type, 0)
                    || p.default.as_ref().is_some_and(|v| !json_matches(v, &p.data_type))
                    || (!p.choices.is_empty()
                        && (!matches!(p.data_type, DataType::String | DataType::Choice)
                            || p.choices.iter().any(|c| c.len() > 1024)))
                {
                    return Err(invalid());
                }
                if p.default.as_ref().is_some_and(|v| {
                    !p.choices.is_empty()
                        && !p.choices.iter().any(|c| Some(c.as_str()) == v.as_str())
                }) {
                    return Err(invalid());
                }
            }
        }
    }
    if signature
        .pins
        .iter()
        .any(|p| signature.pins.iter().any(|q| p.key != q.key && p.key == q.name))
    {
        return Err(invalid());
    }
    if (signature.node_type == NodeType::Function && (exec_in, exec_out) != (1, 1))
        || (signature.node_type == NodeType::Pure && (exec_in, exec_out) != (0, 0))
    {
        return Err(invalid());
    }
    Ok(())
}

pub fn validate_instance(
    node: &Node,
    signature: &NodeSignature,
    binding: &Value,
) -> Result<(), String> {
    validate_bound_instance(node, signature, binding, BINDING_KEY)
}
pub fn validate_bound_instance(
    node: &Node,
    signature: &NodeSignature,
    binding: &Value,
    key: &str,
) -> Result<(), String> {
    if node.data.get(key) != Some(binding) {
        return Err("Addon node package identity changed or is missing; recreate explicitly".into());
    }
    if node.kind != signature.kind
        || node.node_type != signature.node_type
        || node.pins.len() != signature.pins.len()
    {
        return Err("Addon node signature changed".into());
    }
    for spec in &signature.pins {
        let matches: Vec<_> = node
            .pins
            .iter()
            .filter(|p| {
                p.pin_type == spec.pin_type
                    && if spec.key.is_empty() {
                        p.name == spec.name && p.key.as_deref().is_none_or(|key| key == spec.name)
                    } else {
                        p.key.as_deref() == Some(&spec.key)
                    }
            })
            .collect();
        if matches.len() != 1 {
            return Err("Addon node has missing or duplicate pins".into());
        }
        let p = matches[0];
        if p.name != spec.name
            || p.pin_type != spec.pin_type
            || p.data_type != spec.data_type
            || p.default != spec.default
            || p.optional != spec.optional
            || p.choices != spec.choices
        {
            return Err("Addon node pin contract differs from package".into());
        }
        if p.pin_type == PinType::DataInput
            && let Some(v) = inline_value(node, p)
            && !(v.is_null() && p.optional)
            && !json_matches(v, &p.data_type)
        {
            return Err("Addon node inline input has the wrong type".into());
        }
    }
    Ok(())
}
