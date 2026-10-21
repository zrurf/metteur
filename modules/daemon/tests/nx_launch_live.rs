//! Real chat -> exact confirmation -> host yield -> interpreter -> original chat.
//! In-process controllable provider only; no network/model fees.
use async_trait::async_trait;
use metteur_daemon::{
    AppState, DaemonResult, DaemonService, Registry, WorkspaceManager,
    continuation::{
        launch::{LaunchState, LaunchStore},
        state::TaskState,
        store::ContinuationStore,
    },
    llm::{LlmClient, LlmClientFactory, LlmResponse, StreamDelta},
};
use metteur_proto::proto::{
    OpenWorkspaceRequest, ResourceRequest, SendChatRequest, daemon_server::Daemon,
};
use metteur_shared::{
    config::Config,
    llm::{ContextManager, GenerationParams, ToolCall, ToolDefinition, Usage},
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio_stream::StreamExt;
use tonic::Request;
use uuid::Uuid;
struct Model {
    blueprint: Uuid,
    parent: AtomicUsize,
    child: AtomicUsize,
    mode: Mode,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Success,
    Failure,
    Stop,
    ReportOnly,
}
#[async_trait]
impl LlmClient for Model {
    fn provider(&self) -> &str {
        "fixture"
    }
    fn model(&self) -> &str {
        "fixture"
    }
    async fn complete(
        &self,
        ctx: &ContextManager,
        _: &GenerationParams,
        tools: &[ToolDefinition],
    ) -> DaemonResult<LlmResponse> {
        let is_node = ctx.messages.iter().any(|m| m.text_content() == "BLUEPRINT_NODE");
        let (text, calls) = if is_node {
            assert!(!tools.iter().any(|t| t.name == "RequestBlueprintRun"));
            if self.mode == Mode::Stop {
                self.child.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                return Ok(LlmResponse {
                    text: "cancelled fixture".into(),
                    thinking: vec![],
                    tool_calls: vec![],
                    usage: Usage {
                        input_tokens: 8,
                        output_tokens: 2,
                        total_tokens: 10,
                        tokens_reported: true,
                        ..Default::default()
                    },
                });
            }
            match self.child.fetch_add(1, Ordering::SeqCst) {
                0 => (
                    String::new(),
                    vec![ToolCall {
                        id: "read-handoff".into(),
                        name: "ReadHandoff".into(),
                        arguments: serde_json::json!({"mode":"summary"}),
                    }],
                ),
                1 => {
                    assert!(
                        ctx.tool_results
                            .iter()
                            .any(|r| r.content.contains("Review local evidence"))
                    );
                    ("node result retained".into(), vec![])
                }
                _ => panic!("duplicate blueprint model invocation"),
            }
        } else {
            match self.parent.fetch_add(1, Ordering::SeqCst) {
                0 => {
                    assert!(tools.iter().any(|t| t.name == "RequestBlueprintRun"));
                    (
                        String::new(),
                        vec![
                            ToolCall {
                                id: "launch".into(),
                                name: "RequestBlueprintRun".into(),
                                arguments: serde_json::json!({"blueprint_id":self.blueprint,"goal":"Review local evidence","summary":"Model-authored plan","constraints":["Keep existing files"],"resume_on_completion":true,"allow_source_read":true}),
                            },
                            ToolCall {
                                id: "batch-tail".into(),
                                name: "TodoRead".into(),
                                arguments: serde_json::json!({}),
                            },
                        ],
                    )
                }
                1 => {
                    assert!(
                        ctx.messages
                            .iter()
                            .any(|m| m.origin == metteur_shared::llm::MessageOrigin::HostEvent)
                    );
                    assert!(ctx.tool_results.iter().any(|r| r.tool_call_id == "batch-tail"));
                    let result =
                        ctx.tool_results.iter().find(|r| r.tool_call_id == "launch").unwrap();
                    let value: serde_json::Value = serde_json::from_str(&result.content).unwrap();
                    (
                        String::new(),
                        vec![ToolCall {
                            id: "read-result".into(),
                            name: "ReadTaskSource".into(),
                            arguments: serde_json::json!({"source_id":value["completion_source"]}),
                        },ToolCall{id:"read-run-proof".into(),name:"ReadRunEvidence".into(),arguments:serde_json::json!({})}],
                    )
                }
                2 => {
                    assert!(ctx.tool_results.iter().any(|r|r.tool_call_id=="read-run-proof"&&r.content.contains("host_checkpoint")&&r.content.contains("not_asserted")));
                    assert!(ctx.tool_results.iter().any(|r| r.content.contains(
                        if self.mode == Mode::Failure {
                            "failed"
                        } else {
                            "local-evidence-42"
                        }
                    )));
                    ("original conversation completed".into(), vec![])
                }
                _ => panic!("duplicate parent completion"),
            }
        };
        Ok(LlmResponse {
            text,
            thinking: vec![],
            tool_calls: calls,
            usage: Usage {
                input_tokens: 8,
                output_tokens: 2,
                total_tokens: 10,
                tokens_reported: true,
                ..Default::default()
            },
        })
    }
    async fn stream(
        &self,
        c: &ContextManager,
        p: &GenerationParams,
        t: &[ToolDefinition],
        delta: &mut (dyn FnMut(StreamDelta) + Send),
    ) -> DaemonResult<LlmResponse> {
        let r = self.complete(c, p, t).await?;
        if !r.text.is_empty() {
            delta(StreamDelta::Text(r.text.clone()));
        }
        Ok(r)
    }
}
#[tokio::test]
async fn confirmed_saved_blueprint_runs_once_and_returns_actual_outputs_to_parent() {
    run_case(Mode::Success).await
}
#[tokio::test]
async fn failed_node_with_unsettled_fence_requires_review_without_replay() {
    run_case(Mode::Failure).await
}
#[tokio::test]
async fn stopped_blueprint_never_automatically_resumes_parent() {
    run_case(Mode::Stop).await
}
#[tokio::test]
async fn report_only_completion_does_not_run_another_model_turn() {
    run_case(Mode::ReportOnly).await
}
async fn run_case(mode: Mode) {
    let root = std::env::temp_dir().join(format!("nx-launch-live-{}", Uuid::new_v4()));
    std::fs::create_dir_all(root.join("blueprints")).unwrap();
    std::fs::write(root.join("evidence.txt"), "local-evidence-42").unwrap();
    let registry = Arc::new(Registry::with_builtins());
    let graph=metteur_shared::dsl::compile_draft_value_with_catalog(&serde_json::json!({"name":"Local review","nodes":{"start":{"kind":"Start"},"read":{"kind":"ReadFile","path":if mode==Mode::Failure{"missing.txt"}else{"evidence.txt"}},"model":{"kind":"CallLLM","prompt":"BLUEPRINT_NODE","model":"fixture","max_tokens":256}},"flow":["start -> read -> model"]}),&registry.authoring_catalog()).unwrap();
    let model = Arc::new(Model {
        blueprint: graph.id,
        parent: AtomicUsize::new(0),
        child: AtomicUsize::new(0),
        mode,
    });
    let mut state = AppState::new(
        WorkspaceManager::new().with_global_config_path(root.join("global.toml")),
        registry,
        Config::default(),
    );
    state.llm_factory = LlmClientFactory::with_override(model.clone());
    let state = Arc::new(state);
    let service = DaemonService::new(state.clone());
    Daemon::open_workspace(
        &service,
        Request::new(OpenWorkspaceRequest {
            path: root.to_string_lossy().into(),
        }),
    )
    .await
    .unwrap();
    let ws = state.workspaces.get(&root).await.unwrap();
    metteur_daemon::storage::blueprint_files::save(
        &ws.db,
        &ws.version_manager,
        &graph,
        "blueprints/review.blueprint",
        &metteur_daemon::storage::blueprint_files::encode_native(&graph).unwrap(),
        None,
    )
    .unwrap();
    let mut stream = Daemon::send_chat(
        &service,
        Request::new(SendChatRequest {
            workspace_path: root.to_string_lossy().into(),
            message: "Run the saved local review".into(),
            options_json:
                serde_json::json!({"provider":"openai-chat","model":"fixture","max_tokens":512})
                    .to_string(),
            ..Default::default()
        }),
    )
    .await
    .unwrap()
    .into_inner();
    let mut saw_wait = false;
    let mut saw_confirmation = false;
    tokio::time::timeout(std::time::Duration::from_secs(15),async{while let Some(e)=stream.next().await{
  let e=e.unwrap();assert_ne!(e.kind,"error","{}",e.content);
  if e.kind=="approval" {let data:serde_json::Value=serde_json::from_str(&e.detail_json).unwrap();let detail:serde_json::Value=serde_json::from_str(&e.content).expect("approval content");
   assert_eq!(detail["request_type"],"blueprint_launch");saw_confirmation=true;
   let request_id=data["request_id"].as_str().expect("approval request id");
   let response=Daemon::confirm_blueprint_launch(&service,Request::new(ResourceRequest{workspace_path:root.to_string_lossy().into(),schema_version:1,payload_json:serde_json::json!({"request_id":request_id,"intent_id":detail["intent_id"],"original_digest":detail["digest"],"allow":true,"resume_on_completion":mode!=Mode::ReportOnly,"allow_source_read":false}).to_string(),..Default::default()})).await.unwrap();assert!(response.into_inner().error_json.is_empty());
  }
  saw_wait|=e.kind=="waiting";
 }}).await.expect("chat stream must close at safe yield");
    assert!(saw_wait && saw_confirmation);
    let store = ContinuationStore::new(ws.db.clone(), &root).unwrap();
    if mode == Mode::Stop {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while model.child.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        let intents = LaunchStore::new(ws.db.clone(), &root).unwrap().list("local", None).unwrap();
        let run = intents[0].1.run.unwrap();
        Daemon::cancel_execution(
            &service,
            Request::new(metteur_proto::proto::CancelRequest {
                workspace_path: root.to_string_lossy().into(),
                run_id: run.to_string(),
            }),
        )
        .await
        .unwrap();
    }
    let task = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let rows = store.list(None).unwrap();
            assert_eq!(rows.len(), 1);
            let r = &rows[0];
            if !matches!(mode,Mode::Stop|Mode::Failure) {
                assert_ne!(r.task.state, TaskState::NeedsAttention, "{:?}", r.task.attention);
            }
            if r.task.state == TaskState::Completed
                || matches!(mode,Mode::Stop|Mode::Failure) && r.task.state == TaskState::NeedsAttention
            {
                break r.clone();
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("real blueprint must finish and resume original parent");
    let launches = LaunchStore::new(ws.db.clone(), &root).unwrap();
    let records = launches.list("local", None).unwrap();
    assert_eq!(records.len(), 1);
    let intent = &records[0].1;
    assert_eq!(intent.state, LaunchState::Terminal);
    assert_eq!(intent.session, task.task.session);
    assert!(intent.confirmation_request.is_some());
    assert_eq!(
        model.parent.load(Ordering::SeqCst),
        if matches!(mode, Mode::Stop | Mode::ReportOnly | Mode::Failure) {
            1
        } else {
            3
        }
    );
    if mode == Mode::Failure {
        assert_eq!(intent.terminal, Some(metteur_daemon::continuation::state::Terminal::Indeterminate));
        assert_eq!(model.child.load(Ordering::SeqCst), 0);
    }
    if mode == Mode::Success {
        assert_eq!(model.child.load(Ordering::SeqCst), 2);
        assert_eq!(task.task.budget.consumed_tokens, 40);
        assert_eq!(task.task.budget.reserved_tokens, 0);
    }
    if mode == Mode::Stop {
        assert!(matches!(
            intent.terminal,
            Some(
                metteur_daemon::continuation::state::Terminal::Cancelled
                    | metteur_daemon::continuation::state::Terminal::Indeterminate
            )
        ));
        tokio::time::sleep(std::time::Duration::from_millis(650)).await;
        assert_eq!(model.parent.load(Ordering::SeqCst), 1);
    }

    let source =
        metteur_daemon::continuation::handoff::HandoffStore::new(ws.db.clone(), &root).unwrap();
    assert!(!source.get(intent.handoff, "local").unwrap().allow_source_read);
    assert!(source.read_source(intent.handoff, "local", 0).is_err());
    let saved =
        metteur_daemon::chat::session::load_thread(&ws.db, &task.task.session).unwrap().unwrap();
    assert_eq!(
        saved
            .context
            .messages
            .iter()
            .any(|m| m.text_content() == "original conversation completed"),
        matches!(mode, Mode::Success)
    );
    if mode==Mode::Success {
        let rows=metteur_daemon::observability::audit::AuditWriter::new(ws.db.clone()).list().unwrap();
        let observations:Vec<_>=rows.iter().filter(|r|r.operation.starts_with("llm.observation.")).collect();
        assert_eq!(observations.iter().filter(|r|r.operation=="llm.observation.begin").count(),5);
        assert_eq!(observations.iter().filter(|r|r.operation=="llm.observation.end").count(),5);
        assert!(observations.iter().any(|r|r.detail["role"]=="blueprint_executor"));
        assert!(!serde_json::to_string(&observations).unwrap().contains("Review local evidence"));
    }

}
