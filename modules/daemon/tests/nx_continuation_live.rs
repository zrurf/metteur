//! Focused real host/DB/OS-job continuation with a controllable in-process model.
//! No provider network or paid API calls.
use async_trait::async_trait;
use metteur_daemon::{
    AppState, DaemonResult, DaemonService, Registry, WorkspaceManager,
    continuation::{state::TaskState, store::ContinuationStore},
    llm::{LlmClient, LlmClientFactory, LlmResponse, StreamDelta},
};
use metteur_proto::proto::{OpenWorkspaceRequest, SendChatRequest, daemon_server::Daemon};
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

struct Script {
    calls: AtomicUsize,
}
#[async_trait]
impl LlmClient for Script {
    fn provider(&self) -> &str {
        "local-fixture"
    }
    fn model(&self) -> &str {
        "fixture"
    }
    async fn complete(
        &self,
        ctx: &ContextManager,
        _: &GenerationParams,
        _: &[ToolDefinition],
    ) -> DaemonResult<LlmResponse> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        let calls = match n {
            0 => vec![ToolCall {
                id: "start-job".into(),
                name: "StartCommand".into(),
                arguments: serde_json::json!({"command":"sleep 0.2"}),
            }],
            1 => {
                let result = ctx
                    .tool_results
                    .iter()
                    .find(|r| r.tool == "StartCommand")
                    .expect("StartCommand result");
                let data: serde_json::Value =
                    serde_json::from_str(&result.content).expect("JSON job result");
                let id = data["job_id"].as_str().expect("real job ID");
                vec![
                    ToolCall {
                        id: "wait-job".into(),
                        name: "AwaitEvents".into(),
                        arguments: serde_json::json!({"sources":[id],"mode":"any","deadline_ms":chrono::Utc::now().timestamp_millis()+10000,"params":{"purpose":"report"}}),
                    },
                    ToolCall {
                        id: "same-batch".into(),
                        name: "TodoRead".into(),
                        arguments: serde_json::json!({}),
                    },
                ]
            }
            2 => {
                assert!(ctx.tool_results.iter().any(|r| r.tool_call_id == "same-batch"));
                assert!(
                    ctx.messages
                        .iter()
                        .any(|m| m.text_content().contains("Host-delivered source event"))
                );
                vec![]
            }
            _ => panic!("duplicate or unexpected model turn"),
        };
        Ok(LlmResponse {
            text: if n == 2 {
                "completed once".into()
            } else {
                String::new()
            },
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
        ctx: &ContextManager,
        p: &GenerationParams,
        t: &[ToolDefinition],
        delta: &mut (dyn FnMut(StreamDelta) + Send),
    ) -> DaemonResult<LlmResponse> {
        let r = self.complete(ctx, p, t).await?;
        if !r.text.is_empty() {
            delta(StreamDelta::Text(r.text.clone()));
        }
        Ok(r)
    }
}
#[cfg(unix)]
#[tokio::test]
async fn real_job_yields_saves_entire_batch_and_resumes_once_in_same_session() {
    let root = std::env::temp_dir().join(format!("nx-m2-live-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let script = Arc::new(Script {
        calls: AtomicUsize::new(0),
    });
    let mut state = AppState::new(
        WorkspaceManager::new().with_global_config_path(root.join("isolated-global.toml")),
        Arc::new(Registry::with_builtins()),
        Config::default(),
    );
    state.llm_factory = LlmClientFactory::with_override(script.clone());
    let state = Arc::new(state);
    let service = DaemonService::new(state.clone());
    Daemon::open_workspace(
        &service,
        Request::new(OpenWorkspaceRequest {
            path: root.to_string_lossy().into_owned(),
        }),
    )
    .await
    .unwrap();
    let ws = state.workspaces.get(&root).await.unwrap();
    ws.config.write().await.sandbox.whitelist = vec!["sleep *".into()];
    let options = serde_json::json!({"provider":"openai-chat","model":"fixture","max_tokens":512,"permission_mode":"sandbox","continuation_policy":{"detached":true,"max_turns":4,"token_limit":2_000_000,"expires_ms":chrono::Utc::now().timestamp_millis()+30000,"purpose":"Report local job result"}});
    let mut stream = Daemon::send_chat(
        &service,
        Request::new(SendChatRequest {
            workspace_path: root.to_string_lossy().into_owned(),
            message: "Run local fixture and wait".into(),
            options_json: options.to_string(),
            ..Default::default()
        }),
    )
    .await
    .unwrap()
    .into_inner();
    let mut waiting = false;
    let mut waiting_task = None;
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while let Some(e) = stream.next().await {
            let e = e.unwrap();
            assert_ne!(e.kind, "error", "{}", e.content);
            waiting |= e.kind == "waiting";
            if e.kind == "waiting" {
                let detail: serde_json::Value = serde_json::from_str(&e.detail_json).unwrap();
                waiting_task = detail["task_id"].as_str().map(str::to_owned);
            }
        }
    })
    .await
    .expect("old chat RPC must close at Waiting");
    assert!(waiting);
    let store = ContinuationStore::new(ws.db.clone(), &root).unwrap();
    let task = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let page = store.list(None).unwrap();
            assert_eq!(page.len(), 1);
            if page[0].task.state == TaskState::Completed {
                break page[0].clone();
            }
            assert_ne!(
                page[0].task.state,
                TaskState::NeedsAttention,
                "{:?}",
                page[0].task.attention
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("event must resume the task");
    assert_eq!(script.calls.load(Ordering::SeqCst), 3);
    assert_eq!(task.task.budget.consumed_tokens, 30);
    assert_eq!(task.task.budget.reserved_tokens, 0);
    assert!(!task.task.budget.unknown_usage);
    assert_eq!(task.task.dispatch_history.len(), 1);
    assert_eq!(waiting_task, Some(task.task.id.to_string()));
    assert!(task.task.events.values().all(|e| e.consumed));
    let saved =
        metteur_daemon::chat::session::load_thread(&ws.db, &task.task.session).unwrap().unwrap();
    assert!(saved.context.messages.iter().any(|m| m.text_content() == "completed once"));
    assert!(saved.transcript.iter().any(|entry| entry.continuation_task == Some(task.task.id)));
}

struct ChildScript {
    parent: AtomicUsize,
    child: AtomicUsize,
}
#[async_trait]
impl LlmClient for ChildScript {
    fn provider(&self) -> &str {
        "local-fixture"
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
        let is_child = ctx.messages.iter().any(|m| m.text_content() == "READ_CHILD");
        let (text, calls) = if is_child {
            self.child.fetch_add(1, Ordering::SeqCst);
            assert!(!tools.iter().any(|t| matches!(
                t.name.as_str(),
                "WriteFile" | "ExecuteCommand" | "StartCommand" | "StartSubAgent" | "AwaitEvents"
            )));
            ("bounded child result".into(), vec![])
        } else {
            match self.parent.fetch_add(1, Ordering::SeqCst) {
                0 => (
                    String::new(),
                    vec![ToolCall {
                        id: "start-child".into(),
                        name: "StartSubAgent".into(),
                        arguments: serde_json::json!({"task":"READ_CHILD"}),
                    }],
                ),
                1 => {
                    let result =
                        ctx.tool_results.iter().find(|r| r.tool == "StartSubAgent").unwrap();
                    let data: serde_json::Value = serde_json::from_str(&result.content).unwrap();
                    let id = data["source_id"].as_str().unwrap();
                    (
                        String::new(),
                        vec![ToolCall {
                            id: "wait-child".into(),
                            name: "AwaitEvents".into(),
                            arguments: serde_json::json!({"sources":[id],"mode":"all","deadline_ms":chrono::Utc::now().timestamp_millis()+10000,"params":{}}),
                        }],
                    )
                }
                2 => {
                    assert!(
                        ctx.messages
                            .iter()
                            .any(|m| m.text_content().contains("Host-delivered source event"))
                    );
                    ("child reported".into(), vec![])
                }
                _ => panic!("duplicate parent continuation"),
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
async fn background_child_uses_host_allowlist_and_root_ledger() {
    let root = std::env::temp_dir().join(format!("nx-child-live-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let model = Arc::new(ChildScript {
        parent: AtomicUsize::new(0),
        child: AtomicUsize::new(0),
    });
    let mut state = AppState::new(
        WorkspaceManager::new().with_global_config_path(root.join("global.toml")),
        Arc::new(Registry::with_builtins()),
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
    let options = serde_json::json!({"provider":"openai-chat","model":"fixture","max_tokens":512,"continuation_policy":{"detached":true,"max_turns":4,"token_limit":2_000_000,"expires_ms":chrono::Utc::now().timestamp_millis()+30000,"purpose":"Read-only child fixture"}});
    let mut stream = Daemon::send_chat(
        &service,
        Request::new(SendChatRequest {
            workspace_path: root.to_string_lossy().into(),
            message: "Run child fixture".into(),
            options_json: options.to_string(),
            ..Default::default()
        }),
    )
    .await
    .unwrap()
    .into_inner();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while let Some(e) = stream.next().await {
            let e = e.unwrap();
            assert_ne!(e.kind, "error", "{}", e.content);
        }
    })
    .await
    .unwrap();
    let ws = state.workspaces.get(&root).await.unwrap();
    let store = ContinuationStore::new(ws.db.clone(), &root).unwrap();
    let task = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let p = store.list(None).unwrap();
            if p[0].task.state == TaskState::Completed {
                break p[0].clone();
            }
            assert_ne!(p[0].task.state, TaskState::NeedsAttention, "{:?}", p[0].task.attention);
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(model.child.load(Ordering::SeqCst), 1);
    assert_eq!(model.parent.load(Ordering::SeqCst), 3);
    assert_eq!(task.task.budget.consumed_tokens, 40);
    assert_eq!(task.task.budget.reserved_tokens, 0);
}

struct FinalScript {
    calls: AtomicUsize,
}
#[async_trait]
impl LlmClient for FinalScript {
    fn provider(&self) -> &str {
        "local-fixture"
    }
    fn model(&self) -> &str {
        "fixture"
    }
    async fn complete(
        &self,
        ctx: &ContextManager,
        _: &GenerationParams,
        _: &[ToolDefinition],
    ) -> DaemonResult<LlmResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert!(
            ctx.messages.iter().any(|m| m.text_content().contains("Host-delivered source event"))
        );
        Ok(LlmResponse {
            text: "timer reported".into(),
            thinking: vec![],
            tool_calls: vec![],
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
        delta(StreamDelta::Text(r.text.clone()));
        Ok(r)
    }
}
#[tokio::test]
async fn persisted_timer_runs_once_without_an_open_chat_stream() {
    use metteur_daemon::{
        chat::{
            session::ChatSessionRecord,
            wait::{MatchMode, WaitIntent},
        },
        continuation::{
            policy::Policy,
            state::Source,
            timer::{Misfire, Rule, Schedule},
        },
    };
    let root = std::env::temp_dir().join(format!("nx-timer-live-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let model = Arc::new(FinalScript {
        calls: AtomicUsize::new(0),
    });
    let mut state = AppState::new(
        WorkspaceManager::new().with_global_config_path(root.join("global.toml")),
        Arc::new(Registry::with_builtins()),
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
    let store = ContinuationStore::new(ws.db.clone(), &root).unwrap();
    let now = chrono::Utc::now();
    let policy = Policy {
        detached: true,
        max_turns: 3,
        token_limit: 100000,
        expires_ms: (now + chrono::Duration::seconds(20)).timestamp_millis() as u64,
        purpose: "Timer fixture".into(),
    };
    let context = ChatSessionRecord::new(now.timestamp_millis() as u64, Default::default());
    let mut task = policy.task(context.session_id, 0);
    let id = task.id;
    let schedule_id = uuid::Uuid::new_v4();
    let source_id = format!("timer:{schedule_id}:1:0");
    task.register_source(Source {
        id: source_id.clone(),
        kind: "timer".into(),
        task: id,
        generation: 1,
        may_write: false,
        terminal: None,
        result: serde_json::Value::Null,
    })
    .unwrap();
    let r = store.create(task).unwrap();
    let r = store
        .update(id, r.revision, |r| {
            r.owner_subject = "local".into();
            r.binding_digest = Some(metteur_daemon::continuation::policy::binding_digest(
                &Config::default(),
                Some("fixture"),
                &Registry::with_builtins(),
            ));
            r.model_options =
                serde_json::json!({"provider":"openai-chat","model":"fixture","max_tokens":512});
            r.policy = Some(policy.clone());
            r.schedules.push(Schedule {
                id: schedule_id,
                generation: 1,
                task: id,
                task_generation: 1,
                rule: Rule::After {
                    deadline: now + chrono::Duration::milliseconds(150),
                },
                next_due: Some(now + chrono::Duration::milliseconds(150)),
                last_emitted: None,
                max_occurrences: 1,
                occurrences: 0,
                expires_at: now + chrono::Duration::seconds(20),
                paused: false,
                cancelled: false,
                confirmed_digest: "fixture-explicit-confirmation".into(),
                misfire: Misfire::CoalesceOnce,
                params: serde_json::json!({}),
            });
            Ok(())
        })
        .unwrap();
    store
        .save_wait(
            id,
            r.revision,
            context,
            WaitIntent {
                sources: vec![source_id],
                mode: MatchMode::Any,
                deadline_ms: policy.expires_ms,
                params: serde_json::json!({}),
            },
            now.timestamp_millis() as u64,
        )
        .unwrap();
    // Exercise the real two-step schedule RPC and edit generation before dispatch.
    let revision = store.get(id).unwrap().unwrap().revision;
    let mut plan = serde_json::json!({"task_id":id,"id":schedule_id,"rule":{"kind":"after","deadline":chrono::Utc::now()+chrono::Duration::milliseconds(200)},"max_occurrences":1,"expires_at":now+chrono::Duration::seconds(20),"misfire":"coalesce_once","params":{"purpose":"edited local fixture"}});
    let request = |plan: &serde_json::Value| {
        Request::new(metteur_proto::proto::ResourceRequest {
            workspace_path: root.to_string_lossy().into(),
            schema_version: 1,
            payload_json: plan.to_string(),
            expected_revision: revision,
            ..Default::default()
        })
    };
    let preview = Daemon::write_schedule(&service, request(&plan)).await.unwrap().into_inner();
    let preview: serde_json::Value = serde_json::from_str(&preview.payload_json).unwrap();
    assert_eq!(store.get(id).unwrap().unwrap().revision, revision);
    plan["confirmed_digest"] = preview["digest"].clone();
    Daemon::write_schedule(&service, request(&plan)).await.unwrap();
    assert_eq!(store.get(id).unwrap().unwrap().schedules[0].generation, 2);
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let r = store.get(id).unwrap().unwrap();
            if r.task.state == TaskState::Completed {
                break r;
            }
            assert_ne!(r.task.state, TaskState::NeedsAttention, "{:?}", r.task.attention);
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(model.calls.load(Ordering::SeqCst), 1);
    assert_eq!(result.schedules[0].occurrences, 1);
    assert!(result.schedules[0].next_due.is_none());
    assert_eq!(result.task.budget.consumed_tokens, 10);
}

struct FailingBatch;
#[async_trait]
impl LlmClient for FailingBatch {
    fn provider(&self) -> &str {
        "local-fixture"
    }
    fn model(&self) -> &str {
        "fixture"
    }
    async fn complete(
        &self,
        _: &ContextManager,
        _: &GenerationParams,
        _: &[ToolDefinition],
    ) -> DaemonResult<LlmResponse> {
        Ok(LlmResponse {
            text: String::new(),
            thinking: vec![],
            tool_calls: vec![
                ToolCall {
                    id: "missing".into(),
                    name: "ReadFile".into(),
                    arguments: serde_json::json!({"path":"never-exists.txt"}),
                },
                ToolCall {
                    id: "after-error".into(),
                    name: "TodoRead".into(),
                    arguments: serde_json::json!({}),
                },
            ],
            usage: Usage {
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
        _: &mut (dyn FnMut(StreamDelta) + Send),
    ) -> DaemonResult<LlmResponse> {
        self.complete(c, p, t).await
    }
}
#[tokio::test]
async fn failed_batch_records_every_tool_result_before_returning_error() {
    let root = std::env::temp_dir().join(format!("nx-failed-batch-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let mut state = AppState::new(
        WorkspaceManager::new().with_global_config_path(root.join("global.toml")),
        Arc::new(Registry::with_builtins()),
        Config::default(),
    );
    state.llm_factory = LlmClientFactory::with_override(Arc::new(FailingBatch));
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
    ws.config.write().await.llm.tool_error_limit = 1;
    let options = serde_json::json!({"provider":"openai-chat","model":"fixture","max_tokens":512,"continuation_policy":{"detached":true,"max_turns":2,"token_limit":2000000,"expires_ms":chrono::Utc::now().timestamp_millis()+30000,"purpose":"Failed batch"}});
    let mut stream = Daemon::send_chat(
        &service,
        Request::new(SendChatRequest {
            workspace_path: root.to_string_lossy().into(),
            message: "Read missing fixture".into(),
            options_json: options.to_string(),
            ..Default::default()
        }),
    )
    .await
    .unwrap()
    .into_inner();
    let mut error = false;
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while let Some(e) = stream.next().await {
            let e = e.unwrap();
            error |= e.kind == "error";
            assert_ne!(e.kind, "waiting");
            assert_ne!(e.kind, "done");
        }
    })
    .await
    .unwrap();
    assert!(error);
    let store = ContinuationStore::new(ws.db.clone(), &root).unwrap();
    let r = store.list(None).unwrap().remove(0);
    assert_eq!(r.task.state, TaskState::NeedsAttention);
    let context = &r.saved_context.unwrap().context;
    for id in ["missing", "after-error"] {
        assert!(
            context.tool_results.iter().any(|result| result.tool_call_id == id),
            "missing result {id}"
        );
    }
}
