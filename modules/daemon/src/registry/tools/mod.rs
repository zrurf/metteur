//! Built-in tools available to blueprints and LLM agents.

pub mod blueprint;
pub mod command;
pub mod context;
pub mod deps;
pub mod edit;
pub mod fs_tools;
pub mod lsp_tools;
pub mod replan;
pub mod result;
pub mod search;
pub mod snapshot;
pub mod subagent;
pub mod todo;
pub mod memory;

use metteur_shared::Value;

/// A helper for reading named or positional arguments from a value list.
pub(crate) struct Args<'a> {
    values: &'a [Value],
}

impl<'a> Args<'a> {
    pub(crate) fn new(values: &'a [Value]) -> Self {
        Self {
            values,
        }
    }

    /// Returns the value for `name` if the first argument is a JSON object,
    /// otherwise the positional value at `index`.
    pub(crate) fn get(&self, name: &str, index: usize) -> Option<Value> {
        if let Some(Value::Json(obj)) = self.values.first() {
            obj.get(name).map(json_to_value)
        } else {
            self.values.get(index).cloned()
        }
    }

    pub(crate) fn string(&self, name: &str, index: usize) -> Option<String> {
        self.get(name, index).and_then(|v| match v {
            Value::String(s) => Some(s),
            Value::Json(j) => j.as_str().map(|s| s.to_string()),
            _ => None,
        })
    }

    /// Returns a boolean argument, accepting booleans embedded in JSON.
    pub(crate) fn bool(&self, name: &str, index: usize) -> Option<bool> {
        self.get(name, index).and_then(|v| match v {
            Value::Bool(b) => Some(b),
            Value::Json(j) => j.as_bool(),
            _ => None,
        })
    }

    /// Returns an integer argument, accepting numbers embedded in JSON.
    pub(crate) fn int(&self, name: &str, index: usize) -> Option<i64> {
        self.get(name, index).and_then(|v| match v {
            Value::Int(i) => Some(i),
            Value::Float(f) => Some(f as i64),
            Value::Json(j) => j.as_i64(),
            _ => None,
        })
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
pub mod continuation;

pub mod launch;

pub mod run_evidence;

pub mod prompt_query;
