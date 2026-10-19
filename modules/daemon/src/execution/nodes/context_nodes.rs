//! Context-manager node executors.
//!
//! These nodes thread [`ContextManager`] values through the blueprint so a
//! conversation context can be created, transformed and inspected without
//! scattering prompt text across node pins.

use std::collections::HashMap;

use async_trait::async_trait;
use metteur_shared::llm::{ContextManager, Message, Role, SystemFragment};
use metteur_shared::{Node, PinId, Value};

use crate::error::DaemonResult;
use crate::execution::context::ExecutionContext;
use crate::registry::NodeExecutor;

use super::{int_input, string_input, string_output, value_input};

/// Reads a context input by pin name.
fn context_input(
    node: &Node,
    inputs: &HashMap<PinId, Value>,
    name: &str,
) -> DaemonResult<ContextManager> {
    let value = value_input(node, inputs, name)?;
    value.as_context().cloned().ok_or_else(|| {
        crate::error::DaemonError::Execution(format!("input {name} is not a context"))
    })
}

/// Writes a context to the `Result` pin.
fn context_output(node: &Node, value: ContextManager) -> DaemonResult<HashMap<PinId, Value>> {
    let pin =
        node.pins.iter().find(|p| p.name == "Result").ok_or_else(|| {
            crate::error::DaemonError::Execution("missing pin Result".to_string())
        })?;
    Ok(HashMap::from([(pin.id, Value::Context(value))]))
}

/// Maps a role choice string to a [`Role`].
fn role_from_choice(raw: &str) -> Role {
    match raw {
        "assistant" => Role::Assistant,
        "tool" => Role::Tool,
        "system" => Role::System,
        _ => Role::User,
    }
}

/// Creates a context from system text and an optional initial prompt.
pub struct ContextCreateExecutor;

#[async_trait]
impl NodeExecutor for ContextCreateExecutor {
    fn kind(&self) -> &str {
        "ContextCreate"
    }

    async fn execute(
        &self,
        node: &Node,
        inputs: &HashMap<PinId, Value>,
        _ctx: &mut ExecutionContext,
    ) -> DaemonResult<HashMap<PinId, Value>> {
        let system = string_input(node, inputs, "System")?;
        let prompt = string_input(node, inputs, "Prompt")?;
        let fragments = if system.is_empty() {
            Vec::new()
        } else {
            vec![SystemFragment {
                priority: 0,
                scope: "node".to_string(),
                content: system,
            }]
        };
        context_output(node, ContextManager::new_from_prompt(fragments, prompt))
    }
}

/// Clones a context so downstream mutations do not leak upstream.
pub struct ContextCloneExecutor;

#[async_trait]
impl NodeExecutor for ContextCloneExecutor {
    fn kind(&self) -> &str {
        "ContextClone"
    }

    async fn execute(
        &self,
        node: &Node,
        inputs: &HashMap<PinId, Value>,
        _ctx: &mut ExecutionContext,
    ) -> DaemonResult<HashMap<PinId, Value>> {
        context_output(node, context_input(node, inputs, "In")?)
    }
}

/// Mixes one message into a context: `Context + text + role -> Result`.
pub struct ContextMergeExecutor;

#[async_trait]
impl NodeExecutor for ContextMergeExecutor {
    fn kind(&self) -> &str {
        "ContextMerge"
    }

    async fn execute(
        &self,
        node: &Node,
        inputs: &HashMap<PinId, Value>,
        ctx: &mut ExecutionContext,
    ) -> DaemonResult<HashMap<PinId, Value>> {
        let mut context = context_input(node, inputs, "Context")?;
        let text = string_input(node, inputs, "Text")?;
        let mut role = role_from_choice(&string_input(node, inputs, "Role")?);
        if let (Some(graph),Some(db),Some(pin))=(&ctx.blueprint,&ctx.workspace_db,node.pins.iter().find(|pin|pin.name=="Text")) {
            let parents=crate::prompt_snapshots::prompt_ancestors(&graph.read(),node.id,pin.id);
            if !parents.is_empty() {
                let store=crate::prompt_snapshots::SelectionStore::new(db.clone(),&ctx.workspace_root).map_err(|error|crate::DaemonError::Execution(error.to_string()))?;
                let mut refs=Vec::new();let mut equal=false;
                for (id,_) in &parents {let snapshot=store.run_snapshot(ctx.run_id,*id).map_err(|error|crate::DaemonError::Execution(error.to_string()))?;equal=snapshot.body==text;refs.push(snapshot.reference);}
                store.record_origin(ctx.run_id,node.id,&text,refs,parents.len()==1 && parents[0].1,equal).map_err(|error|crate::DaemonError::Execution(error.to_string()))?;
                role=metteur_shared::llm::Role::User;
            }
        }
        context.push_message(Message::text(role, text));
        context_output(node, context)
    }
}

/// Keeps only the messages of one role: `Context + Role -> Result`.
pub struct ContextFilterExecutor;

#[async_trait]
impl NodeExecutor for ContextFilterExecutor {
    fn kind(&self) -> &str {
        "ContextFilter"
    }

    async fn execute(
        &self,
        node: &Node,
        inputs: &HashMap<PinId, Value>,
        _ctx: &mut ExecutionContext,
    ) -> DaemonResult<HashMap<PinId, Value>> {
        let mut context = context_input(node, inputs, "Context")?;
        let role = role_from_choice(&string_input(node, inputs, "Role")?);
        context.messages.retain(|m| m.role == role);
        context_output(node, context)
    }
}

/// Keeps the most recent `keep` messages of a context.
pub struct ContextTrimExecutor;

#[async_trait]
impl NodeExecutor for ContextTrimExecutor {
    fn kind(&self) -> &str {
        "ContextTrim"
    }

    async fn execute(
        &self,
        node: &Node,
        inputs: &HashMap<PinId, Value>,
        _ctx: &mut ExecutionContext,
    ) -> DaemonResult<HashMap<PinId, Value>> {
        let mut context = context_input(node, inputs, "Context")?;
        let keep = int_input(node, inputs, "Keep")?.max(0) as usize;
        if context.messages.len() > keep {
            let drop = context.messages.len() - keep;
            context.messages.drain(..drop);
        }
        context_output(node, context)
    }
}

/// Releases tool results from a context (deterministic counterpart of the
/// `ReleaseContext` tool): `Context + selector -> Result + Released + Freed`.
pub struct ContextReleaseExecutor;

#[async_trait]
impl NodeExecutor for ContextReleaseExecutor {
    fn kind(&self) -> &str {
        "ContextRelease"
    }

    async fn execute(
        &self,
        node: &Node,
        inputs: &HashMap<PinId, Value>,
        _ctx: &mut ExecutionContext,
    ) -> DaemonResult<HashMap<PinId, Value>> {
        let mut context = context_input(node, inputs, "Context")?;
        let query = release_query(node, inputs)?;
        let report = context.release(&query);
        let mut outputs = context_output(node, context)?;
        for (name, value) in [
            ("Released", report.released as i64),
            ("Freed", report.estimated_tokens as i64),
        ] {
            if let Some(pin) = node.pins.iter().find(|p| p.name == name) {
                outputs.insert(pin.id, Value::Int(value));
            }
        }
        Ok(outputs)
    }
}

/// Reads the release selector from connected inputs, falling back to `node.data`.
fn release_query(
    node: &Node,
    inputs: &HashMap<PinId, Value>,
) -> DaemonResult<metteur_shared::llm::ReleaseQuery> {
    use metteur_shared::llm::ReleaseQuery;
    /// A connected list input wins; otherwise the inline `data` value is used.
    fn list(node: &Node, inputs: &HashMap<PinId, Value>, pin: &str, key: &str) -> Vec<String> {
        let from_pin: Option<Vec<String>> = match super::value_input(node, inputs, pin).ok() {
            Some(Value::List(items)) => Some(
                items.iter().filter_map(|item| item.as_str().map(String::from)).collect(),
            ),
            Some(Value::String(single)) if !single.is_empty() => Some(vec![single.clone()]),
            _ => None,
        };
        if let Some(values) = from_pin {
            return values;
        }
        node.data
            .get(key)
            .and_then(|value| value.as_array())
            .map(|items| {
                items.iter().filter_map(|item| item.as_str().map(String::from)).collect()
            })
            .unwrap_or_default()
    }

    let query = ReleaseQuery {
        paths: list(node, inputs, "Paths", "paths"),
        patterns: list(node, inputs, "Patterns", "patterns"),
        tools: list(node, inputs, "Tools", "tools"),
        all: node.data.get("all").and_then(|value| value.as_bool()).unwrap_or(false),
        keep_recent: node.data.get("keep_recent").and_then(|value| value.as_u64()).unwrap_or(0)
            as usize,
    };
    if !query.selects_anything() {
        return Err(crate::error::DaemonError::Execution(
            "ContextRelease needs one of: Paths, Patterns, Tools, all".to_string(),
        ));
    }
    Ok(query)
}

/// Dumps a context's messages as text for inspection.
pub struct ContextToTextExecutor;

#[async_trait]
impl NodeExecutor for ContextToTextExecutor {
    fn kind(&self) -> &str {
        "ContextToText"
    }

    async fn execute(
        &self,
        node: &Node,
        inputs: &HashMap<PinId, Value>,
        _ctx: &mut ExecutionContext,
    ) -> DaemonResult<HashMap<PinId, Value>> {
        let context = context_input(node, inputs, "In")?;
        let mut out = String::new();
        for fragment in &context.system_fragments {
            out.push_str(&format!("[system] {}\n", fragment.content));
        }
        for message in &context.messages {
            let role = format!("{:?}", message.role).to_ascii_lowercase();
            out.push_str(&format!("[{role}] {}\n", message.text_content()));
        }
        string_output(node, "Result", out)
    }
}
