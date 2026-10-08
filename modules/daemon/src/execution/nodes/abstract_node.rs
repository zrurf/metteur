//! Abstract node executor: plans and executes a nested blueprint at runtime.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use metteur_shared::llm::{ContextManager, Message, Role};
use metteur_shared::{Blueprint, Node, PinId, Value};

use crate::error::{DaemonError, DaemonResult};
use crate::execution::context::ExecutionContext;
use crate::execution::interpreter::{ExecutionEvent, Interpreter};
use crate::execution::react::{ReactOptions, run_react};
use crate::observability::anon::Anonymizer;
use crate::registry::{NodeExecutor, Registry};

/// A node whose execution plans a sub-blueprint with an LLM and runs it.
///
/// The planner prompt lists available node kinds and tools plus a strict JSON
/// output contract. The produced blueprint is deanonymized, validated and,
/// on success, executed atomically by a nested [`Interpreter`]. Invalid
/// output is retried once with the validator errors appended to the prompt.
pub struct AbstractExecutor;

#[async_trait]
impl NodeExecutor for AbstractExecutor {
    fn kind(&self) -> &str {
        "Abstract"
    }

    async fn execute(
        &self,
        node: &Node,
        _inputs: &HashMap<PinId, Value>,
        ctx: &mut ExecutionContext,
    ) -> DaemonResult<HashMap<PinId, Value>> {
        let description = node
            .data
            .get("description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| {
                DaemonError::Execution("Abstract node requires a 'description'".to_string())
            })?;
        let max_expand_depth =
            node.data.get("max_expand_depth").and_then(|v| v.as_u64()).unwrap_or(1) as u32;

        let anonymizer = build_anonymizer(ctx).await;
        // The planner emits JSON and calls no tools, so it gets the environment
        // (workspace path, platform) rather than the full harness prompt.
        let system = vec![crate::harness::environment_fragment(ctx).await];
        let mut context =
            ContextManager::new_from_prompt(system, build_plan_prompt(ctx, &description));
        let mut errors = String::new();

        for attempt in 0..=1u32 {
            if attempt > 0 {
                context.push_message(Message::text(
                    Role::User,
                    format!(
                        "The previous blueprint was invalid:\n{errors}\n\
                         Fix every issue and output ONLY the corrected JSON object."
                    ),
                ));
            }
            let opts = plan_options(node);
            let outcome = run_react(ctx, context, &opts).await?;
            context = outcome.context;
            // Restore real secrets before validating the generated blueprint.
            let raw = anonymizer.deanonymize(&outcome.text).await;
            match validate_blueprint(&raw, &ctx.registry, ctx.depth, max_expand_depth) {
                Ok(blueprint) => {
                    expand_blueprint(ctx, blueprint).await?;
                    return Ok(HashMap::new());
                }
                Err(err) => errors = err,
            }
        }

        Err(DaemonError::Execution(format!(
            "abstract node produced an invalid blueprint: {errors}"
        )))
    }
}

/// Builds ReAct options for the planning pass (tools disabled).
///
/// The `mock_text` data key implies the mock provider so tests can script the
/// generated blueprint offline.
fn plan_options(node: &Node) -> ReactOptions {
    let data = &node.data;
    let string = |key: &str| {
        data.get(key).and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(|s| s.to_string())
    };
    let mock_text = string("mock_text");
    let provider = string("provider").unwrap_or_else(|| {
        if mock_text.is_some() {
            "mock".to_string()
        } else {
            "openai-chat".to_string()
        }
    });
    ReactOptions {
        provider,
        model: string("model"),
        // Planning never calls tools: an empty allowed set disables them all.
        allowed_tools: Some(HashSet::new()),
        label: "Abstract".to_string(),
        mock_text,
        ..Default::default()
    }
}

/// Builds the planner prompt describing kinds, tools and the output contract.
fn build_plan_prompt(ctx: &ExecutionContext, description: &str) -> String {
    let kinds = ctx.registry.node_kinds().join(", ");
    let tools: Vec<serde_json::Value> = ctx
        .registry
        .tools()
        .iter()
        .map(|t| {
            serde_json::json!({
                "name": t.name(),
                "description": t.description(),
                "parameters": t.parameters(),
            })
        })
        .collect();
    let tools_json = serde_json::to_string_pretty(&tools).unwrap_or_else(|_| "[]".to_string());
    format!(
        "Design a Metteur blueprint that accomplishes the task below.\n\n\
         Available node kinds: {kinds}\n\n\
         Available tools:\n{tools_json}\n\n\
         Each node follows the shared Node JSON schema: \
         {{\"id\":uuid,\"node_type\":\"Event\"|\"Function\"|\"Pure\"|\"Control\",\"kind\":string,\
         \"position\":[x,y],\"pins\":[{{\"id\":uuid,\"name\":string,\"pin_type\":\"ExecInput\"|\
         \"ExecOutput\"|\"DataInput\"|\"DataOutput\",\"data_type\":\"Void\"|\"Bool\"|\"Int\"|\
         \"Float\"|\"String\"|\"List\"|\"Json\"}}],\"data\":object}}.\n\
         Each edge follows: {{\"id\":uuid,\"source_node\":uuid,\"source_pin\":uuid,\
         \"target_node\":uuid,\"target_pin\":uuid}}.\n\n\
         Output ONLY a JSON object: {{\"id\":uuid,\"name\":string,\"nodes\":[{{...shared Node JSON schema...}}],\"edges\":[...],\"entry_node_id\":uuid}}\n\n\
         Task:\n{description}"
    )
}

/// Validates a generated blueprint, returning all violations found.
fn validate_blueprint(
    raw: &str,
    registry: &Registry,
    depth: u32,
    max_expand_depth: u32,
) -> Result<Blueprint, String> {
    let blueprint: Blueprint = match serde_json::from_str(raw) {
        Ok(bp) => bp,
        Err(err) => return Err(format!("blueprint is not valid JSON: {err}")),
    };
    let mut errors: Vec<String> = Vec::new();
    if blueprint.node(blueprint.entry_node_id).is_none() {
        errors.push(format!("entry_node_id {} does not exist", blueprint.entry_node_id));
    }
    for edge in &blueprint.edges {
        if blueprint.node(edge.source_node).is_none() {
            errors.push(format!("edge source node {} does not exist", edge.source_node));
        }
        if blueprint.node(edge.target_node).is_none() {
            errors.push(format!("edge target node {} does not exist", edge.target_node));
        }
        if blueprint.pin(edge.source_pin).is_none() {
            errors.push(format!("edge source pin {} does not exist", edge.source_pin));
        }
        if blueprint.pin(edge.target_pin).is_none() {
            errors.push(format!("edge target pin {} does not exist", edge.target_pin));
        }
    }
    for node in &blueprint.nodes {
        if registry.node_executor(&node.kind).is_none() {
            errors.push(format!("unknown node kind '{}'", node.kind));
        }
        if node.kind == "Abstract" && depth >= max_expand_depth {
            errors.push("nested Abstract nodes exceed the expansion depth limit".to_string());
        }
    }
    if errors.is_empty() {
        Ok(blueprint)
    } else {
        Err(errors.join("; "))
    }
}

/// Executes the validated blueprint in a nested interpreter and forwards its
/// events as outer message events.
async fn expand_blueprint(ctx: &mut ExecutionContext, blueprint: Blueprint) -> DaemonResult<()> {
    ctx.audit(
        "abstract.expand",
        serde_json::json!({
            "depth": ctx.depth + 1,
            "nodes": blueprint.nodes.len(),
            "blueprint": serde_json::to_value(&blueprint)
                .unwrap_or(serde_json::Value::Null),
        }),
    );

    let event_tx = ctx.events.clone();
    let label = "Abstract";
    let mut interpreter =
        Interpreter::new(ctx.registry.clone(), ctx.llm_factory.clone(), ctx.workspace_root.clone())
            .with_user(ctx.user.clone())
            .with_transaction_log(ctx.transaction_log.clone());
    if let Some(audit) = &ctx.audit {
        interpreter = interpreter.with_audit(audit.clone());
    }
    if let Some(config) = &ctx.config {
        interpreter = interpreter.with_config(config.clone());
    }
    if let Some(metrics) = &ctx.metrics {
        interpreter = interpreter.with_metrics(metrics.clone());
    }
    if let Some(db) = &ctx.workspace_db {
        interpreter = interpreter.with_workspace_db(db.clone());
    }
    if let Some(db) = &ctx.global_db {
        interpreter = interpreter.with_global_db(db.clone());
    }
    // Capability handles are forwarded so the expansion runs under the same
    // approvals, version tracking and job supervision as the outer graph.
    interpreter = interpreter
        .with_jobs(ctx.jobs.clone())
        .with_addon_fragments(ctx.addon_fragments.clone());
    if let Some(source) = &ctx.lsp_source {
        interpreter = interpreter.with_lsp_source(source.clone());
    }
    if let Some(lsp) = &ctx.lsp {
        interpreter = interpreter.with_lsp(lsp.clone());
    }
    if let Some(approvals) = &ctx.approvals {
        interpreter = interpreter.with_inherited_approvals(approvals.clone());
    }
    if let Some(vm) = &ctx.version_manager {
        interpreter = interpreter.with_version_manager(vm.clone());
    }

    // The parent's control flags are shared, not re-created: an expansion is
    // the longest-running node kind, and with private flags a cancel or pause
    // would stay inert for its whole duration. The interrupt bus is
    // deliberately *not* shared — the nested blueprint gets its own handle,
    // because sharing the parent's would make the nested run re-execute the
    // outer graph.
    let pause_requested = ctx.pause_requested.clone();
    let cancel_requested = ctx.cancel_requested.clone();
    let node_count = blueprint.nodes.len();
    let shared = Arc::new(parking_lot::RwLock::new(blueprint));
    ctx.tree_ops.push(crate::execution::TreeOp::SpawnChild {
        kind: crate::execution::TreeNodeKind::Function("abstract".to_string()),
        label: format!("abstract expansion ({node_count} nodes)"),
    });
    let nested_events = match interpreter
        .run_with_control(&shared, None, pause_requested, cancel_requested)
        .await
    {
        Ok(events) => events,
        Err(err) => {
            ctx.tree_ops.push(crate::execution::TreeOp::FinishCurrent {
                status: crate::execution::TreeNodeStatus::Failed(err.to_string()),
            });
            return Err(err);
        }
    };
    ctx.tree_ops.push(crate::execution::TreeOp::FinishCurrent {
        status: crate::execution::TreeNodeStatus::Done,
    });
    if let Some(tx) = &event_tx {
        for event in nested_events {
            let _ = tx.send(forward_event(event, label));
        }
        let _ = tx.send(ExecutionEvent::Message {
            node_id: ctx.current_node,
            message: format!("{label}: abstract expanded {node_count} nodes"),
        });
    }
    Ok(())
}

/// Wraps a nested event into an outer message event.
fn forward_event(event: ExecutionEvent, label: &str) -> ExecutionEvent {
    match event {
        ExecutionEvent::NodeStarted {
            node_id,
        } => ExecutionEvent::Message {
            node_id,
            message: format!("{label}: started node {node_id}"),
        },
        ExecutionEvent::NodeFinished {
            node_id,
        } => ExecutionEvent::Message {
            node_id,
            message: format!("{label}: finished node {node_id}"),
        },
        ExecutionEvent::Message {
            node_id,
            message,
        } => ExecutionEvent::Message {
            node_id,
            message: format!("{label}: message node {node_id}: {message}"),
        },
        review @ ExecutionEvent::Oversight { .. } => review,
        approval @ ExecutionEvent::ApprovalRequested {
            ..
        } => approval,
        usage @ ExecutionEvent::ContextUsage {
            ..
        } => usage,
        todos @ ExecutionEvent::Todos {
            ..
        } => todos,
        job @ ExecutionEvent::Job {
            ..
        } => job,
        data @ ExecutionEvent::NodeData {
            ..
        } => data,
    }
}

/// Builds an anonymizer from the merged configuration.
async fn build_anonymizer(ctx: &ExecutionContext) -> Anonymizer {
    crate::execution::react::build_anonymizer(ctx).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use metteur_shared::{DataType, Edge, NodeType, Pin, PinType};
    use uuid::Uuid;

    fn new_ctx(workspace_root: std::path::PathBuf) -> ExecutionContext {
        ExecutionContext::new(
            std::sync::Arc::new(crate::registry::Registry::with_builtins()),
            crate::llm::LlmClientFactory::new(),
            workspace_root,
        )
    }

    fn pin(id: Uuid, name: &str, pin_type: PinType, data_type: DataType) -> Pin {
        Pin::data(name.to_string(), pin_type, data_type, id)
    }

    /// Builds a nested blueprint: Start(Path) -> CallLLM(mock) ->
    /// Tool(WriteFile). Running it writes the mocked LLM answer into a file.
    fn nested_blueprint(path: &str, content: &str) -> Blueprint {
        let start = Uuid::new_v4();
        let llm = Uuid::new_v4();
        let tool = Uuid::new_v4();

        let start_exec = Uuid::new_v4();
        let start_path = Uuid::new_v4();
        let llm_exec_in = Uuid::new_v4();
        let llm_exec_out = Uuid::new_v4();
        let llm_result = Uuid::new_v4();
        let tool_exec_in = Uuid::new_v4();
        let tool_path = Uuid::new_v4();
        let tool_content = Uuid::new_v4();
        let tool_result = Uuid::new_v4();

        Blueprint {
            id: Uuid::new_v4(),
            name: "generated".to_string(),
            nodes: vec![
                Node {
                    id: start,
                    node_type: NodeType::Event,
                    kind: "Start".to_string(),
                    position: (0.0, 0.0),
                    pins: vec![
                        pin(start_exec, "Exec", PinType::ExecOutput, DataType::Void),
                        pin(start_path, "Path", PinType::DataOutput, DataType::String),
                    ],
                    data: serde_json::json!({ "Path": path }),
                },
                Node {
                    id: llm,
                    node_type: NodeType::Function,
                    kind: "CallLLM".to_string(),
                    position: (0.0, 0.0),
                    pins: vec![
                        pin(llm_exec_in, "Exec", PinType::ExecInput, DataType::Void),
                        pin(llm_exec_out, "Exec", PinType::ExecOutput, DataType::Void),
                        pin(llm_result, "Result", PinType::DataOutput, DataType::String),
                    ],
                    data: serde_json::json!({
                        "provider": "mock",
                        "mock_text": content,
                        "prompt": "answer",
                    }),
                },
                Node {
                    id: tool,
                    node_type: NodeType::Function,
                    kind: "Tool".to_string(),
                    position: (0.0, 0.0),
                    pins: vec![
                        pin(tool_exec_in, "Exec", PinType::ExecInput, DataType::Void),
                        pin(tool_path, "path", PinType::DataInput, DataType::String),
                        pin(tool_content, "content", PinType::DataInput, DataType::String),
                        pin(tool_result, "Result", PinType::DataOutput, DataType::String),
                    ],
                    data: serde_json::json!({ "tool_name": "WriteFile" }),
                },
            ],
            edges: vec![
                Edge {
                    id: Uuid::new_v4(),
                    source_node: start,
                    source_pin: start_exec,
                    target_node: llm,
                    target_pin: llm_exec_in,
                },
                Edge {
                    id: Uuid::new_v4(),
                    source_node: llm,
                    source_pin: llm_exec_out,
                    target_node: tool,
                    target_pin: tool_exec_in,
                },
                Edge {
                    id: Uuid::new_v4(),
                    source_node: start,
                    source_pin: start_path,
                    target_node: tool,
                    target_pin: tool_path,
                },
                Edge {
                    id: Uuid::new_v4(),
                    source_node: llm,
                    source_pin: llm_result,
                    target_node: tool,
                    target_pin: tool_content,
                },
            ],
            entry_node_id: start,
        }
    }

    fn abstract_node(data: serde_json::Value) -> Node {
        Node {
            id: Uuid::new_v4(),
            node_type: NodeType::Function,
            kind: "Abstract".to_string(),
            position: (0.0, 0.0),
            pins: Vec::new(),
            data,
        }
    }

    #[tokio::test]
    async fn requires_description() {
        let mut ctx = new_ctx(std::env::temp_dir());
        let node = abstract_node(serde_json::json!({}));
        let result = AbstractExecutor.execute(&node, &HashMap::new(), &mut ctx).await;
        assert!(matches!(result, Err(DaemonError::Execution(msg)) if msg.contains("description")));
    }

    #[tokio::test]
    async fn expands_valid_mock_blueprint() {
        let workspace = std::env::temp_dir().join(format!("metteur-abstract-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&workspace).unwrap();
        let blueprint = nested_blueprint("out.txt", "nested-value-42");
        let mut ctx = new_ctx(workspace.clone());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        ctx.events = Some(tx);

        let node = abstract_node(serde_json::json!({
            "description": "store the answer",
            "provider": "mock",
            "mock_text": serde_json::to_string(&blueprint).unwrap(),
        }));
        AbstractExecutor.execute(&node, &HashMap::new(), &mut ctx).await.unwrap();

        // The nested blueprint ran and produced the expected data value.
        let written = std::fs::read_to_string(workspace.join("out.txt")).unwrap();
        assert_eq!(written, "nested-value-42");

        // Nested events are forwarded as outer message events.
        let mut messages = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let ExecutionEvent::Message {
                message,
                ..
            } = event
            {
                messages.push(message);
            }
        }
        assert!(messages.iter().any(|m| m.contains("abstract expanded 3 nodes")));
        assert!(messages.iter().any(|m| m.contains("finished node")));

        std::fs::remove_dir_all(&workspace).ok();
    }

    #[tokio::test]
    async fn invalid_output_rejected_after_retry() {
        let mut ctx = new_ctx(std::env::temp_dir());
        let node = abstract_node(serde_json::json!({
            "description": "impossible",
            "provider": "mock",
            "mock_text": "this is definitely not json",
        }));
        let result = AbstractExecutor.execute(&node, &HashMap::new(), &mut ctx).await;
        assert!(
            matches!(result, Err(DaemonError::Execution(msg)) if msg.contains("invalid blueprint"))
        );
    }

    #[tokio::test]
    async fn unknown_kind_and_missing_entry_are_rejected() {
        let blueprint = r#"{"id":"11111111-1111-1111-1111-111111111111","name":"bad",
            "nodes":[],"edges":[],
            "entry_node_id":"22222222-2222-2222-2222-222222222222"}"#;
        let registry = Registry::with_builtins();
        let err = validate_blueprint(blueprint, &registry, 0, 1).unwrap_err();
        assert!(err.contains("does not exist"));
        assert!(err.contains("entry_node_id"));
    }
}
