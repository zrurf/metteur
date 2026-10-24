//! Integration tests for the Round 16 context behaviors of the ReAct loop:
//! model-driven release, duplicate-read supersession, iteration-budget
//! wrap-up and SubAgent context inheritance.

use std::collections::HashMap;
use std::sync::Arc;

use metteur_daemon::error::DaemonResult;
use metteur_daemon::execution::context::{ContextOp, ExecutionContext};
use metteur_daemon::execution::react::{ReactOptions, run_react};
use metteur_daemon::llm::{
    LlmClient, LlmClientFactory, LlmResponse, MockClient, MockStep, StreamDelta,
};
use metteur_daemon::registry::{Registry, Tool};
use metteur_shared::llm::{ContextManager, GenerationParams, ReleaseQuery, ToolDefinition};
use metteur_shared::{ToolResultLifetime, Value};

/// A tool that queues a full release, exercising the tool→loop channel.
struct ReleaseEverything;

#[async_trait::async_trait]
impl Tool for ReleaseEverything {
    fn name(&self) -> &str {
        "ReleaseEverything"
    }

    fn description(&self) -> &str {
        "Queues a release of every retained tool result."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object" })
    }

    async fn call(&self, _args: &[Value], ctx: &mut ExecutionContext) -> DaemonResult<Value> {
        ctx.context_ops.push(ContextOp::Release(ReleaseQuery {
            all: true,
            ..Default::default()
        }));
        Ok(Value::String("release queued".to_string()))
    }
}

/// A read-only tool that declares a complete read of `read.txt`.
struct FullReader;

#[async_trait::async_trait]
impl Tool for FullReader {
    fn name(&self) -> &str {
        "FullReader"
    }

    fn description(&self) -> &str {
        "Reads read.txt in full."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object" })
    }

    fn read_only(&self) -> bool {
        true
    }

    fn lifetime(&self) -> ToolResultLifetime {
        ToolResultLifetime::Persistent
    }

    async fn call(&self, _args: &[Value], ctx: &mut ExecutionContext) -> DaemonResult<Value> {
        ctx.note_full_read([ctx.workspace_root.join("read.txt")]);
        Ok(Value::String("full body".to_string()))
    }
}

/// A read-only tool returning a fixed payload.
struct FixedReader {
    payload: String,
}

#[async_trait::async_trait]
impl Tool for FixedReader {
    fn name(&self) -> &str {
        "FixedReader"
    }

    fn description(&self) -> &str {
        "Returns a fixed payload."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object" })
    }

    fn read_only(&self) -> bool {
        true
    }

    fn lifetime(&self) -> ToolResultLifetime {
        ToolResultLifetime::Persistent
    }

    async fn call(&self, _args: &[Value], _ctx: &mut ExecutionContext) -> DaemonResult<Value> {
        Ok(Value::String(self.payload.clone()))
    }
}

/// A client whose first answer is a redacted reasoning block plus a tool call,
/// then a plain final answer.
#[derive(Default)]
struct RedactedReasoner {
    calls: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl LlmClient for RedactedReasoner {
    fn provider(&self) -> &str {
        "redacted"
    }

    fn model(&self) -> &str {
        "redacted-model"
    }

    async fn complete(
        &self,
        _ctx: &ContextManager,
        _params: &GenerationParams,
        _tools: &[ToolDefinition],
    ) -> DaemonResult<LlmResponse> {
        if self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            Ok(LlmResponse {
                text: String::new(),
                thinking: vec![metteur_daemon::llm::ThinkingBlock {
                    text: String::new(),
                    signature: None,
                    redacted: Some("opaque-blob".to_string()),
                }],
                tool_calls: vec![call("FixedReader", serde_json::json!({}))],
                usage: Default::default(),
            })
        } else {
            Ok(LlmResponse {
                text: "done".to_string(),
                thinking: Vec::new(),
                tool_calls: Vec::new(),
                usage: Default::default(),
            })
        }
    }

    async fn stream(
        &self,
        ctx: &ContextManager,
        params: &GenerationParams,
        tools: &[ToolDefinition],
        _on_delta: &mut (dyn FnMut(StreamDelta) + Send),
    ) -> DaemonResult<LlmResponse> {
        self.complete(ctx, params, tools).await
    }
}

/// Wraps a mock client to record what each request carried.
struct RecordingClient {
    inner: MockClient,
    tool_counts: Arc<std::sync::Mutex<Vec<usize>>>,
    messages: Arc<std::sync::Mutex<Vec<Vec<String>>>>,
    fragments: Arc<std::sync::Mutex<Vec<Vec<String>>>>,
}

impl RecordingClient {
    fn new(steps: Vec<MockStep>) -> Self {
        Self {
            inner: MockClient::new(steps),
            tool_counts: Arc::new(std::sync::Mutex::new(Vec::new())),
            messages: Arc::new(std::sync::Mutex::new(Vec::new())),
            fragments: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }
}

#[async_trait::async_trait]
impl LlmClient for RecordingClient {
    fn provider(&self) -> &str {
        "recording"
    }

    fn model(&self) -> &str {
        "recording-model"
    }

    async fn complete(
        &self,
        ctx: &ContextManager,
        params: &GenerationParams,
        tools: &[ToolDefinition],
    ) -> DaemonResult<LlmResponse> {
        self.tool_counts.lock().unwrap().push(tools.len());
        self.messages.lock().unwrap().push(ctx.messages.iter().map(|m| m.text_content()).collect());
        self.fragments
            .lock()
            .unwrap()
            .push(ctx.system_fragments.iter().map(|f| f.scope.clone()).collect());
        self.inner.complete(ctx, params, tools).await
    }

    async fn stream(
        &self,
        ctx: &ContextManager,
        params: &GenerationParams,
        tools: &[ToolDefinition],
        on_delta: &mut (dyn FnMut(StreamDelta) + Send),
    ) -> DaemonResult<LlmResponse> {
        let response = self.complete(ctx, params, tools).await?;
        on_delta(StreamDelta::Text(response.text.clone()));
        Ok(response)
    }
}

fn temp_root(tag: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("metteur-ctx-{tag}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn context_with(root: &std::path::Path, tools: Vec<Arc<dyn Tool>>) -> ExecutionContext {
    let registry = Registry::with_builtins();
    for tool in tools {
        registry.register_tool(tool);
    }
    ExecutionContext::new(Arc::new(registry), LlmClientFactory::new(), root.to_path_buf())
}

/// A context that resolves a model, so the factory override is consulted for
/// nested runs (a SubAgent builds its client through the same resolution).
fn context_with_model(root: &std::path::Path, tools: Vec<Arc<dyn Tool>>) -> ExecutionContext {
    use metteur_shared::config::{Config, LlmConfig, LlmModelConfig};
    let mut ctx = context_with(root, tools);
    ctx.config = Some(Arc::new(tokio::sync::RwLock::new(Config {
        llm: LlmConfig {
            default_model: Some("recording".to_string()),
            models: HashMap::from([(
                "recording".to_string(),
                LlmModelConfig {
                    model_id: "recording-model".to_string(),
                    api_type: "openai-chat".to_string(),
                    ..Default::default()
                },
            )]),
            ..Default::default()
        },
        ..Default::default()
    })));
    ctx
}

fn call(name: &str, arguments: serde_json::Value) -> metteur_shared::llm::ToolCall {
    metteur_shared::llm::ToolCall {
        id: format!("call_{}", uuid::Uuid::new_v4()),
        name: name.to_string(),
        arguments,
    }
}

fn scripted() -> ReactOptions {
    ReactOptions {
        provider: "openai-chat".to_string(),
        model: Some("mock-model".to_string()),
        ..Default::default()
    }
}

/// Runs the loop with a scripted mock client.
async fn run_with_steps(
    ctx: &mut ExecutionContext,
    steps: Vec<MockStep>,
    opts: ReactOptions,
) -> DaemonResult<metteur_daemon::execution::react::ReactOutcome> {
    let client: Arc<dyn LlmClient> = Arc::new(MockClient::new(steps));
    ctx.llm_factory = LlmClientFactory::with_override(client);
    let context = ContextManager::new_from_prompt(vec![], "start");
    run_react(ctx, context, &opts).await
}

#[tokio::test]
async fn a_release_request_frees_earlier_results_before_the_next_turn() {
    let root = temp_root("release");
    let mut ctx = context_with(
        &root,
        vec![
            Arc::new(FixedReader {
                payload: "payload".to_string(),
            }),
            Arc::new(ReleaseEverything),
        ],
    );
    let steps = vec![
        MockStep::Tools(vec![call("FixedReader", serde_json::json!({}))]),
        MockStep::Tools(vec![call("ReleaseEverything", serde_json::json!({}))]),
        MockStep::Text("done".to_string()),
    ];
    let outcome = run_with_steps(&mut ctx, steps, scripted()).await.unwrap();
    let messages: Vec<String> = outcome.context.messages.iter().map(|m| m.text_content()).collect();
    assert!(
        messages.iter().any(|text| text.contains("[released: FixedReader result")),
        "the earlier read must be released: {messages:?}"
    );
    assert!(messages.iter().any(|text| text.contains("released 1 tool result(s)")));
    // The release never targets the result that requested it.
    assert!(messages.iter().any(|text| text.contains("release queued")));
    assert!(outcome.context.tool_results.iter().all(|result| result.tool != "FixedReader"));
}

#[tokio::test]
async fn a_second_full_read_supersedes_the_first_one() {
    let root = temp_root("dedup");
    std::fs::write(root.join("read.txt"), "body").unwrap();
    let mut ctx = context_with(&root, vec![Arc::new(FullReader)]);
    let steps = vec![
        MockStep::Tools(vec![call("FullReader", serde_json::json!({ "n": 1 }))]),
        MockStep::Tools(vec![call("FullReader", serde_json::json!({ "n": 2 }))]),
        MockStep::Text("done".to_string()),
    ];
    let outcome = run_with_steps(&mut ctx, steps, scripted()).await.unwrap();
    let messages: Vec<String> = outcome.context.messages.iter().map(|m| m.text_content()).collect();
    assert!(messages.iter().any(|text| text.contains("[superseded: a later read")), "{messages:?}");
    // Only the newer read is still tracked as a result.
    assert_eq!(outcome.context.tool_results.len(), 1);
}

#[tokio::test]
async fn read_deduplication_can_be_disabled() {
    let root = temp_root("dedup-off");
    std::fs::write(root.join("read.txt"), "body").unwrap();
    let mut ctx = context_with(&root, vec![Arc::new(FullReader)]);
    let steps = vec![
        MockStep::Tools(vec![call("FullReader", serde_json::json!({ "n": 1 }))]),
        MockStep::Tools(vec![call("FullReader", serde_json::json!({ "n": 2 }))]),
        MockStep::Text("done".to_string()),
    ];
    let opts = ReactOptions {
        dedup_reads: false,
        ..scripted()
    };
    let outcome = run_with_steps(&mut ctx, steps, opts).await.unwrap();
    assert!(!outcome.context.messages.iter().any(|m| m.text_content().contains("superseded")));
    assert_eq!(outcome.context.tool_results.len(), 2);
}

#[tokio::test]
async fn the_low_budget_notice_is_emitted_once() {
    let root = temp_root("notice");
    let mut ctx = context_with(
        &root,
        vec![Arc::new(FixedReader {
            payload: "data".to_string(),
        })],
    );
    let mut steps: Vec<MockStep> = (0..4)
        .map(|index| MockStep::Tools(vec![call("FixedReader", serde_json::json!({ "n": index }))]))
        .collect();
    steps.push(MockStep::Text("done".to_string()));
    let opts = ReactOptions {
        max_iterations: 5,
        ..scripted()
    };
    let outcome = run_with_steps(&mut ctx, steps, opts).await.unwrap();
    let notices = outcome
        .context
        .messages
        .iter()
        .filter(|m| m.text_content().contains("iteration budget: 1 of 5"))
        .count();
    assert_eq!(notices, 1, "the notice must not repeat");
    assert_eq!(outcome.text, "done");
}

#[tokio::test]
async fn the_wrap_up_turn_runs_without_tools() {
    let root = temp_root("wrapup");
    let mut ctx = context_with(
        &root,
        vec![Arc::new(FixedReader {
            payload: "data".to_string(),
        })],
    );
    let recorder = Arc::new(RecordingClient::new(vec![
        MockStep::Tools(vec![call("FixedReader", serde_json::json!({}))]),
        MockStep::Text("final answer".to_string()),
    ]));
    let counts = recorder.tool_counts.clone();
    ctx.llm_factory = LlmClientFactory::with_override(recorder);
    let opts = ReactOptions {
        max_iterations: 1,
        ..scripted()
    };
    let context = ContextManager::new_from_prompt(vec![], "start");
    let outcome = run_react(&mut ctx, context, &opts).await.unwrap();
    assert_eq!(outcome.text, "final answer");
    let counts = counts.lock().unwrap();
    assert_eq!(counts.len(), 2, "one budgeted turn and one wrap-up turn");
    assert!(counts[0] > 0, "the budgeted turn keeps its tools");
    assert_eq!(counts[1], 0, "the wrap-up turn must offer no tools");
    assert!(
        outcome
            .context
            .messages
            .iter()
            .any(|m| m.text_content().contains("iteration budget exhausted"))
    );
}

#[tokio::test]
async fn a_wrap_up_turn_that_calls_a_tool_fails_the_run() {
    let root = temp_root("wrapup-fail");
    let mut ctx = context_with(
        &root,
        vec![Arc::new(FixedReader {
            payload: "data".to_string(),
        })],
    );
    // The model keeps asking for tools even after the budget is gone.
    let steps = vec![
        MockStep::Tools(vec![call("FixedReader", serde_json::json!({ "n": 0 }))]),
        MockStep::Tools(vec![call("FixedReader", serde_json::json!({ "n": 1 }))]),
    ];
    let opts = ReactOptions {
        max_iterations: 1,
        ..scripted()
    };
    let error = run_with_steps(&mut ctx, steps, opts).await.unwrap_err();
    assert!(error.to_string().contains("without a final answer"), "{error}");
}

#[tokio::test]
async fn a_subagent_inherits_the_conversation_when_asked() {
    let root = temp_root("inherit");
    let mut ctx = context_with_model(
        &root,
        vec![Arc::new(metteur_daemon::registry::tools::subagent::SpawnSubAgent)],
    );
    let recorder = Arc::new(RecordingClient::new(vec![
        MockStep::Tools(vec![call(
            "SpawnSubAgent",
            serde_json::json!({
                "task": "child task",
                "inherit_context": true,
            }),
        )]),
        // The sub-agent consumes the second step; the parent resumes after it.
        MockStep::Text("child answer".to_string()),
        MockStep::Text("parent answer".to_string()),
    ]));
    let seen = recorder.messages.clone();
    ctx.llm_factory = LlmClientFactory::with_override(recorder);
    let context = ContextManager::new_from_prompt(vec![], "parent question");
    let outcome = run_react(&mut ctx, context, &scripted()).await.unwrap();
    assert_eq!(outcome.text, "parent answer");
    let seen = seen.lock().unwrap();
    // The child's request is the second one; it must carry the parent's prose.
    assert!(seen.len() >= 2, "{seen:?}");
    let child = &seen[1];
    // The parent's trailing user turn and the task share one turn: providers
    // reject two user messages in a row.
    assert_eq!(child.len(), 1, "{child:?}");
    assert!(child[0].contains("parent question"), "{child:?}");
    assert!(child[0].contains("child task"), "{child:?}");
}

#[tokio::test]
async fn a_subagent_does_not_inherit_by_default() {
    let root = temp_root("no-inherit");
    let mut ctx = context_with_model(
        &root,
        vec![Arc::new(metteur_daemon::registry::tools::subagent::SpawnSubAgent)],
    );
    let recorder = Arc::new(RecordingClient::new(vec![
        MockStep::Tools(vec![call("SpawnSubAgent", serde_json::json!({ "task": "child task" }))]),
        MockStep::Text("child answer".to_string()),
        MockStep::Text("parent answer".to_string()),
    ]));
    let seen = recorder.messages.clone();
    ctx.llm_factory = LlmClientFactory::with_override(recorder);
    let context = ContextManager::new_from_prompt(vec![], "parent question");
    run_react(&mut ctx, context, &scripted()).await.unwrap();
    let seen = seen.lock().unwrap();
    let child = &seen[1];
    assert_eq!(child, &vec!["child task".to_string()], "a fresh sub-agent starts from the task");
}

#[tokio::test]
async fn release_context_is_a_registered_tool() {
    // The registry is what makes the tool visible to the model and to DSL
    // compilation; a missing registration would silently disable the feature.
    let registry = Registry::with_builtins();
    let names = registry.tool_names();
    assert!(names.contains(&"ReleaseContext".to_string()), "{names:?}");
    let map: HashMap<&str, ()> = names.iter().map(|name| (name.as_str(), ())).collect();
    assert!(map.contains_key("ReleaseContext"));
}

#[tokio::test]
async fn the_context_release_node_is_registered() {
    let registry = Registry::with_builtins();
    assert!(registry.node_executor("ContextRelease").is_some());
}

#[tokio::test]
async fn window_pressure_releases_results_before_summarizing() {
    use metteur_shared::config::{Config, LlmConfig, LlmModelConfig};
    let root = temp_root("relief");
    let mut ctx = context_with(
        &root,
        vec![Arc::new(FixedReader {
            // Sized so three results overfill the window but one fits with room
            // to spare under either margin (2% exact, 15% heuristic): the test
            // must exercise relief, never fall through to summarizing.
            payload: "p".repeat(1600),
        })],
    );
    ctx.config = Some(Arc::new(tokio::sync::RwLock::new(Config {
        llm: LlmConfig {
            default_model: Some("tiny".to_string()),
            auto_release_keep_results: 1,
            models: HashMap::from([(
                "tiny".to_string(),
                LlmModelConfig {
                    model_id: "recording-model".to_string(),
                    api_type: "openai-chat".to_string(),
                    context_window_input: Some(1200),
                    context_window_output: Some(50),
                    ..Default::default()
                },
            )]),
            ..Default::default()
        },
        ..Default::default()
    })));
    let recorder = Arc::new(RecordingClient::new(vec![
        MockStep::Tools(vec![call("FixedReader", serde_json::json!({ "n": 0 }))]),
        MockStep::Tools(vec![call("FixedReader", serde_json::json!({ "n": 1 }))]),
        MockStep::Tools(vec![call("FixedReader", serde_json::json!({ "n": 2 }))]),
        MockStep::Text("done".to_string()),
    ]));
    let calls = recorder.messages.clone();
    ctx.llm_factory = LlmClientFactory::with_override(recorder);
    let opts = ReactOptions {
        model: Some("tiny".to_string()),
        ..scripted()
    };
    let context = ContextManager::new_from_prompt(vec![], "start");
    let outcome = run_react(&mut ctx, context, &opts).await.unwrap();
    assert_eq!(outcome.text, "done");
    let messages: Vec<String> = outcome.context.messages.iter().map(|m| m.text_content()).collect();
    assert!(
        messages.iter().any(|text| text.contains("released")),
        "automatic relief must release results first: {messages:?}"
    );
    // Releasing is deterministic: the four model turns are the only calls.
    assert_eq!(calls.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn the_context_release_node_trims_a_context_value() {
    use metteur_shared::llm::{ContextManager, ToolResult, ToolResultLifetime};
    use metteur_shared::{DataType, Node, NodeType, Pin, PinType};

    let root = temp_root("node-release");
    let ctx = context_with(&root, Vec::new());
    let registry = ctx.registry.clone();
    let context_pin = Pin {
        id: uuid::Uuid::new_v4(),
        name: "Context".to_string(),
        pin_type: PinType::DataInput,
        data_type: DataType::Context,
        ..Default::default()
    };
    let result_pin = Pin {
        id: uuid::Uuid::new_v4(),
        name: "Result".to_string(),
        pin_type: PinType::DataOutput,
        data_type: DataType::Context,
        ..Default::default()
    };
    let released_pin = Pin {
        id: uuid::Uuid::new_v4(),
        name: "Released".to_string(),
        pin_type: PinType::DataOutput,
        data_type: DataType::Int,
        ..Default::default()
    };
    let node = Node {
        id: uuid::Uuid::new_v4(),
        node_type: NodeType::Pure,
        kind: "ContextRelease".to_string(),
        position: (0.0, 0.0),
        pins: vec![context_pin.clone(), result_pin.clone(), released_pin.clone()],
        data: serde_json::json!({ "all": true }),
    };
    let mut context = ContextManager::new_from_prompt(vec![], "start");
    for index in 0..3 {
        context.mix_in_tool_result(ToolResult {
            tool_call_id: format!("call_{index}"),
            tool: "ReadFile".to_string(),
            content: "body".to_string(),
            timestamp: index,
            lifetime: ToolResultLifetime::Persistent,
            paths: Vec::new(),
        });
    }
    let inputs = std::collections::HashMap::from([(context_pin.id, Value::Context(context))]);
    let mut executor_ctx = context_with(&root, Vec::new());
    let executor = registry.node_executor("ContextRelease").expect("registered");
    let outputs = executor.execute(&node, &inputs, &mut executor_ctx).await.unwrap();
    assert_eq!(outputs.get(&released_pin.id), Some(&Value::Int(3)));
    let trimmed = outputs.get(&result_pin.id).and_then(|v| v.as_context()).unwrap();
    assert!(trimmed.tool_results.is_empty());
    assert!(trimmed.messages.iter().any(|m| m.text_content().contains("[released:")));

    // The selector also works from inline `data` (patterns + keep_recent), the
    // shape a DSL or Inspector edit produces.
    let mut context = ContextManager::new_from_prompt(vec![], "start");
    for (index, path) in ["src/main.rs", "src/lib.rs", "docs/readme.md"].iter().enumerate() {
        context.mix_in_tool_result(ToolResult {
            tool_call_id: format!("call_{index}"),
            tool: "ReadFile".to_string(),
            content: "body".to_string(),
            timestamp: index as u64,
            lifetime: ToolResultLifetime::Persistent,
            paths: vec![std::path::PathBuf::from(path)],
        });
    }
    let pattern_node = Node {
        data: serde_json::json!({ "patterns": ["src/**"], "keep_recent": 1 }),
        ..node.clone()
    };
    let inputs = std::collections::HashMap::from([(context_pin.id, Value::Context(context))]);
    let outputs = executor.execute(&pattern_node, &inputs, &mut executor_ctx).await.unwrap();
    // Both `src/**` results are released; the newest result stays regardless.
    assert_eq!(outputs.get(&released_pin.id), Some(&Value::Int(2)));
    let trimmed = outputs.get(&result_pin.id).and_then(|v| v.as_context()).unwrap();
    let kept: Vec<&str> = trimmed.tool_results.iter().map(|r| r.tool_call_id.as_str()).collect();
    assert_eq!(kept, vec!["call_2"]);
}

#[tokio::test]
async fn reasoning_streams_before_the_answer_and_is_reported_with_it() {
    use metteur_daemon::execution::react::ReactEvent;

    let root = temp_root("reasoning");
    let mut ctx = context_with(&root, Vec::new());
    let client: Arc<dyn LlmClient> = Arc::new(MockClient::new(vec![MockStep::Reasoning {
        thinking: "weighing options".to_string(),
        text: "the answer".to_string(),
    }]));
    ctx.llm_factory = LlmClientFactory::with_override(client);
    let context = ContextManager::new_from_prompt(vec![], "start");
    let mut deltas = Vec::new();
    let mut events = Vec::new();
    let mut on_delta = |delta: StreamDelta| deltas.push(delta);
    let mut on_event = |event: ReactEvent| events.push(event);
    let outcome = metteur_daemon::execution::react::run_react_streaming(
        &mut ctx,
        context,
        &scripted(),
        Some(&mut on_delta),
        &mut on_event,
    )
    .await
    .unwrap();
    assert_eq!(
        deltas,
        vec![
            StreamDelta::Reasoning("weighing options".to_string()),
            StreamDelta::Text("the answer".to_string()),
        ]
    );
    match &events[0] {
        ReactEvent::Assistant {
            text,
            reasoning,
        } => {
            assert_eq!(text, "the answer");
            assert_eq!(reasoning, "weighing options");
        }
        other => panic!("unexpected event: {other:?}"),
    }
    assert_eq!(outcome.text, "the answer");
}

#[tokio::test]
async fn a_subagent_inherits_the_harness_exactly_once() {
    let root = temp_root("inherit-fragments");
    let mut ctx = context_with_model(
        &root,
        vec![Arc::new(metteur_daemon::registry::tools::subagent::SpawnSubAgent)],
    );
    let recorder = Arc::new(RecordingClient::new(vec![
        MockStep::Tools(vec![call(
            "SpawnSubAgent",
            serde_json::json!({ "task": "child task", "inherit_context": true }),
        )]),
        MockStep::Text("child answer".to_string()),
        MockStep::Text("parent answer".to_string()),
    ]));
    let scopes = recorder.fragments.clone();
    ctx.llm_factory = LlmClientFactory::with_override(recorder);
    // The parent context already carries its harness sections.
    let mut context = ContextManager::new_from_prompt(vec![], "parent question");
    let harness = metteur_daemon::harness::fragments(&ctx).await;
    metteur_daemon::harness::HarnessPrompt::apply(&mut context, harness);
    run_react(&mut ctx, context, &scripted()).await.unwrap();
    let scopes = scopes.lock().unwrap();
    let child = &scopes[1];
    let identity = child.iter().filter(|scope| *scope == "harness.identity").count();
    assert_eq!(identity, 1, "harness sections must not be duplicated: {child:?}");
}

#[tokio::test]
async fn a_redacted_reasoning_block_survives_in_the_context() {
    let root = temp_root("redacted");
    let mut ctx = context_with(&root, vec![Arc::new(FixedReader {
        payload: "data".to_string(),
    })]);
    ctx.llm_factory = LlmClientFactory::with_override(Arc::new(RedactedReasoner::default()));
    let opts = scripted();
    let context = ContextManager::new_from_prompt(vec![], "start");
    let outcome = run_react(&mut ctx, context, &opts).await.unwrap();
    assert_eq!(outcome.text, "done");
    let assistant = outcome
        .context
        .messages
        .iter()
        .find(|m| m.role == metteur_shared::llm::Role::Assistant && !m.tool_calls.is_empty())
        .expect("assistant turn with tool calls");
    // The redacted payload is replayed verbatim, and no empty text block is
    // emitted alongside it.
    assert!(
        matches!(
            assistant.content.as_slice(),
            [metteur_shared::llm::ContentBlock::RedactedThinking { data }] if data == "opaque-blob"
        ),
        "{:?}",
        assistant.content
    );
}

#[tokio::test]
async fn release_context_refuses_outside_a_conversation() {
    use metteur_daemon::registry::tools::context::ReleaseContext;

    let root = temp_root("no-loop");
    // A blueprint Tool node has no loop that could drain the request.
    let mut ctx = context_with(&root, Vec::new());
    let error = ReleaseContext
        .call(&[Value::Json(serde_json::json!({ "all": true }))], &mut ctx)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("ContextRelease node"), "{error}");
    assert!(ctx.context_ops.is_empty());

    // Inside a loop the same call is accepted.
    ctx.in_react_loop = true;
    ReleaseContext
        .call(&[Value::Json(serde_json::json!({ "all": true }))], &mut ctx)
        .await
        .unwrap();
    assert_eq!(ctx.context_ops.len(), 1);
}

/// A context whose sandbox is disabled, so the model can run commands.
fn context_with_jobs(tag: &str) -> ExecutionContext {
    use metteur_shared::config::SandboxConfig;
    let mut ctx = context_with(&temp_root(tag), Vec::new());
    ctx.config = Some(Arc::new(tokio::sync::RwLock::new(
        metteur_shared::config::Config {
            sandbox: SandboxConfig {
                enabled: false,
                ..Default::default()
            },
            ..Default::default()
        },
    )));
    ctx
}

#[tokio::test]
async fn delivered_command_results_do_not_wake_subsequent_model_turns() {
    use metteur_daemon::registry::tools::command::{ExecuteCommand, StartCommand, WaitJob};
    let mut ctx = context_with_jobs("delivered-command");
    // A deterministic blueprint baseline predates the model loop.
    ExecuteCommand
        .call(&[Value::Json(serde_json::json!({"command":"echo baseline"}))], &mut ctx)
        .await
        .unwrap();
    let job = StartCommand
        .call(&[Value::Json(serde_json::json!({"command":"echo waited"}))], &mut ctx)
        .await
        .unwrap();
    let Value::Json(job) = job else {
        panic!("expected job payload")
    };
    WaitJob
        .call(&[Value::Json(serde_json::json!({"job_id":job["job_id"]}))], &mut ctx)
        .await
        .unwrap();
    let outcome = run_with_steps(
        &mut ctx,
        vec![
            MockStep::Tools(vec![call(
                "ExecuteCommand",
                serde_json::json!({"command":"echo checked"}),
            )]),
            MockStep::Text("finished once".to_string()),
        ],
        scripted(),
    )
    .await
    .unwrap();
    assert_eq!(outcome.text, "finished once");
    assert!(
        !outcome
            .context
            .messages
            .iter()
            .any(|m| m.text_content().contains("[engine] background job"))
    );
    assert!(ctx.jobs.first_finished(ctx.run_id, &[]).is_none());
}

#[tokio::test]
async fn a_finished_job_wakes_the_parked_turn() {
    let mut ctx = context_with_jobs("auto-wake");
    let steps = vec![
        MockStep::Tools(vec![call(
            "StartCommand",
            serde_json::json!({ "command": "echo woke-up" }),
        )]),
        // The model answers without calling a tool, but its job is still there:
        // the loop must park instead of ending the turn.
        MockStep::Text("waiting for the command".to_string()),
        MockStep::Text("final answer".to_string()),
    ];
    let outcome = run_with_steps(&mut ctx, steps, scripted()).await.unwrap();
    assert_eq!(outcome.text, "final answer");
    let messages: Vec<String> =
        outcome.context.messages.iter().map(|m| m.text_content()).collect();
    assert!(
        messages.iter().any(|text| text.contains("[engine] background job")),
        "the engine must report the job: {messages:?}"
    );
    assert!(
        messages.iter().any(|text| text.contains("woke-up")),
        "the notice carries the job output: {messages:?}"
    );
    // The model's "I will wait" turn is kept, and the notice is a user turn.
    assert!(messages.iter().any(|text| text == "waiting for the command"));
    assert!(
        outcome
            .context
            .messages
            .iter()
            .any(|m| m.role == metteur_shared::llm::Role::User
                && m.text_content().contains("[engine] background job"))
    );
}

#[tokio::test]
async fn auto_wake_can_be_disabled() {
    use metteur_shared::config::{Config, ExecutionConfig, SandboxConfig};
    let mut ctx = context_with_jobs("no-auto-wake");
    ctx.config = Some(Arc::new(tokio::sync::RwLock::new(Config {
        sandbox: SandboxConfig {
            enabled: false,
            ..Default::default()
        },
        execution: ExecutionConfig {
            job_auto_wake: false,
            ..Default::default()
        },
        ..Default::default()
    })));
    let steps = vec![
        MockStep::Tools(vec![call(
            "StartCommand",
            serde_json::json!({ "command": "echo not-awaited" }),
        )]),
        MockStep::Text("done already".to_string()),
    ];
    let outcome = run_with_steps(&mut ctx, steps, scripted()).await.unwrap();
    assert_eq!(outcome.text, "done already");
    let messages: Vec<String> =
        outcome.context.messages.iter().map(|m| m.text_content()).collect();
    assert!(
        !messages.iter().any(|text| text.contains("[engine] background job")),
        "with auto-wake off the turn ends immediately: {messages:?}"
    );
}

#[tokio::test]
async fn a_running_job_parks_the_turn_until_it_finishes() {
    let mut ctx = context_with_jobs("park");
    // A command that takes about a second; the turn must wait for it rather
    // than ending and leaving the result unread.
    let command = if cfg!(windows) {
        "ping -n 3 127.0.0.1 > nul & echo slow-done"
    } else {
        "sleep 2; echo slow-done"
    };
    let steps = vec![
        MockStep::Tools(vec![call(
            "StartCommand",
            serde_json::json!({ "command": command }),
        )]),
        MockStep::Text("waiting".to_string()),
        MockStep::Text("after the wait".to_string()),
    ];
    let outcome = run_with_steps(&mut ctx, steps, scripted()).await.unwrap();
    assert_eq!(outcome.text, "after the wait");
    let messages: Vec<String> =
        outcome.context.messages.iter().map(|m| m.text_content()).collect();
    assert!(
        messages.iter().any(|text| text.contains("slow-done")),
        "the parked turn receives the completed output: {messages:?}"
    );
    assert!(!ctx.jobs.any_running(ctx.run_id), "nothing is left running");
}

/// Providers require every tool result to follow the assistant turn that
/// requested it *immediately*: an assistant message in between makes the
/// request invalid (OpenAI and DeepSeek both reject it with a 400).
fn assert_tool_results_follow_their_call(context: &ContextManager) {
    for (index, message) in context.messages.iter().enumerate() {
        if message.tool_calls.is_empty() {
            continue;
        }
        let expected: Vec<&str> =
            message.tool_calls.iter().map(|call| call.id.as_str()).collect();
        let following: Vec<&str> = context.messages[index + 1..]
            .iter()
            .take_while(|next| next.role == metteur_shared::llm::Role::Tool)
            .filter_map(|next| next.tool_call_id.as_deref())
            .collect();
        assert_eq!(
            following, expected,
            "tool results must directly follow their call: {:?}",
            context.messages
        );
    }
}

#[tokio::test]
async fn text_alongside_tool_calls_keeps_the_message_sequence_valid() {
    let root = temp_root("sequencing");
    let mut ctx = context_with(&root, vec![Arc::new(FixedReader {
        payload: "data".to_string(),
    })]);
    // Models usually narrate the call ("let me read that file") before making
    // it; that text belongs to the same assistant turn as the tool calls.
    let steps = vec![
        MockStep::ToolsWithText {
            text: "let me look".to_string(),
            calls: vec![call("FixedReader", serde_json::json!({ "n": 0 }))],
        },
        MockStep::ToolsWithText {
            text: "one more".to_string(),
            calls: vec![call("FixedReader", serde_json::json!({ "n": 1 }))],
        },
        MockStep::Text("done".to_string()),
    ];
    let outcome = run_with_steps(&mut ctx, steps, scripted()).await.unwrap();
    assert_eq!(outcome.text, "done");
    assert_tool_results_follow_their_call(&outcome.context);
    // The narration is preserved exactly once, inside the assistant turn.
    let narrated = outcome
        .context
        .messages
        .iter()
        .filter(|message| message.text_content() == "let me look")
        .count();
    assert_eq!(narrated, 1, "the assistant text must not be duplicated");
}
