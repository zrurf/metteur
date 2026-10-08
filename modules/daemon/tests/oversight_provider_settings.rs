use axum::{Json, Router, routing::post};
use metteur_daemon::{
    execution::{
        CheckpointSink, DbCheckpointSink, ExecutionCheckpoint,
        context::ExecutionContext,
        react::{ReactOptions, run_react},
    },
    llm::LlmClientFactory,
    oversight::{conversation, requests, review, scheduler},
    registry::Registry,
    storage::persistence::Db,
};
use metteur_shared::{
    config::{Config, LlmModelConfig, oversight::OversightConfig},
    llm::ContextManager,
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

// Only loopback HTTP is used. Each role gets its own provider, database and workspace.
#[tokio::test]
async fn provider_settings_reach_real_second_rounds_without_sharing_authority() {
    for api in ["openai-chat", "openai-responses", "anthropic"] {
        for (model, replay, thinking) in [
            ("ordinary", Some(true), "local reasoning"),
            ("DeepSeek-test", Some(false), "local reasoning"),
            ("vendor/DEEPSEEK-test", None, "local reasoning"),
            ("ordinary", None, "local reasoning"),
            ("DeepSeek-test", None, ""),
        ] {
            for role in ["main", "supervisor", "concierge"] {
                let captured = Arc::new(Mutex::new(Vec::<Value>::new()));
                let requests = captured.clone();
                let app = Router::new().route(match api {
                    "openai-chat" => "/chat/completions",
                    "openai-responses" => "/responses",
                    _ => "/messages",
                }, post(move |Json(request): Json<Value>| {
                    let mut captured = requests.lock().unwrap();
                    let first = captured.is_empty();
                    captured.push(request);
                    let tool = first && role != "concierge";
                    let name = if role == "main" { "ReadFile" } else { "ReadRunStats" };
                    let args = if role == "main" { json!({"path":"evidence.txt"}) } else { json!({}) };
                    let final_text = if role == "supervisor" {
                        r#"{"verdict":"ok","summary":"Observed local evidence"}"#
                    } else if role == "concierge" {
                        r#"{"kind":"answer","text":"Observed local evidence"}"#
                    } else { "Observed local evidence" };
                    let result = match api {
                        "openai-chat" => {
                            let mut message = json!({"content":if tool { "" } else { final_text }, "reasoning_content":thinking});
                            if tool { message["tool_calls"] = json!([{"id":"local-call","type":"function","function":{"name":name,"arguments":args.to_string()}}]); }
                            json!({"choices":[{"message":message}],"usage":{"prompt_tokens":10,"completion_tokens":5}})
                        }
                        "openai-responses" => {
                            let mut output = vec![json!({"type":"reasoning","summary":[{"type":"summary_text","text":thinking}]})];
                            output.push(if tool { json!({"type":"function_call","call_id":"local-call","name":name,"arguments":args.to_string()}) } else { json!({"type":"message","content":[{"type":"output_text","text":final_text}]}) });
                            json!({"output":output,"usage":{"input_tokens":10,"output_tokens":5}})
                        }
                        _ => {
                            let mut content = vec![];
                            if !thinking.is_empty() { content.push(json!({"type":"thinking","thinking":thinking,"signature":"local-signature"})); }
                            content.push(if tool { json!({"type":"tool_use","id":"local-call","name":name,"input":args}) } else { json!({"type":"text","text":final_text}) });
                            json!({"content":content,"usage":{"input_tokens":10,"output_tokens":5}})
                        }
                    };
                    async move { Json(result) }
                }));
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let endpoint = format!("http://{}", listener.local_addr().unwrap());
                let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
                let root =
                    std::env::temp_dir().join(format!("provider-settings-{}", Uuid::new_v4()));
                std::fs::create_dir_all(&root).unwrap();
                std::fs::write(root.join("evidence.txt"), "local evidence").unwrap();
                let db = Db::open(&root.join("db")).unwrap();
                let run = Uuid::new_v4();
                DbCheckpointSink::new(db.clone(), run)
                    .write(&ExecutionCheckpoint::running(run, Uuid::new_v4(), 0))
                    .unwrap();
                let mut config = Config::default();
                config.llm.thinking_budget_tokens = 1024;
                config.llm.prompt_cache = false;
                config.llm.models.insert(
                    "alias".into(),
                    LlmModelConfig {
                        api_type: api.into(),
                        api_endpoint: endpoint,
                        api_key: "synthetic".into(),
                        model_id: model.into(),
                        replay_reasoning: replay,
                        ..Default::default()
                    },
                );
                config
                    .extra
                    .insert("oversight".into(), json!({"model":"alias","concierge_model":"alias"}));
                let factory = LlmClientFactory::new();
                match role {
                    "main" => {
                        let mut ctx = ExecutionContext::new(
                            Arc::new(Registry::with_builtins()),
                            factory,
                            root.clone(),
                        );
                        ctx.config = Some(Arc::new(tokio::sync::RwLock::new(config.clone())));
                        // Also exercise the single configured model path, with no explicit/default key.
                        run_react(
                            &mut ctx,
                            ContextManager::new_from_prompt(vec![], "Read evidence"),
                            &ReactOptions {
                                max_tokens: Some(2048),
                                allowed_tools: Some(["ReadFile".into()].into()),
                                ..Default::default()
                            },
                        )
                        .await
                        .unwrap();
                    }
                    "supervisor" => {
                        scheduler::initialize(
                            &db,
                            run,
                            OversightConfig::from_config(&config).unwrap(),
                        )
                        .unwrap();
                        requests::receive(
                            &db,
                            run,
                            run,
                            Uuid::new_v4(),
                            "Inspect progress",
                            requests::Intent {
                                category: requests::Category::Query,
                                note: "Inspect progress".into(),
                            },
                        )
                        .unwrap();
                        let pending = scheduler::claim(&db, run, 1).unwrap().unwrap();
                        let (key, client) = review::client(&config, &factory).unwrap();
                        let result =
                            review::evaluate(&db, &pending, &config, &key, client.as_ref())
                                .await
                                .unwrap();
                        assert_eq!(
                            result.status,
                            scheduler::Status::Completed,
                            "{api}/{model}/{replay:?}"
                        );
                        assert!(result.actual_action_refs.is_empty());
                    }
                    _ => {
                        let (key, client) = conversation::client(&config, &factory).unwrap();
                        let conversation_id = Uuid::new_v4();
                        for _ in 0..2 {
                            let id = Uuid::new_v4();
                            conversation::begin(&db, run, conversation_id, id, "Inspect progress")
                                .unwrap();
                            let turn =
                                conversation::answer(&db, run, id, &config, &key, client.as_ref())
                                    .await
                                    .unwrap();
                            assert_eq!(turn.state, "answered");
                        }
                        config.extra.insert("oversight".into(), json!({}));
                        config.llm.default_model = Some("alias".into());
                        assert!(conversation::client(&config, &factory).is_err());
                    }
                }
                {
                    let captured = captured.lock().unwrap();
                    assert_eq!(captured.len(), 2, "{api}/{role}/{model}/{replay:?}");
                    for request in captured.iter() {
                        assert_eq!(request["model"], model);
                        if role == "concierge" {
                            assert!(request.get("tools").is_none());
                        }
                        if role == "supervisor" {
                            let tools = request["tools"].to_string();
                            assert!(tools.contains("ReadRunStats"));
                            assert!(
                                !tools.contains("Shell")
                                    && !tools.contains("RespondApproval")
                                    && !tools.contains("ReadFile")
                            );
                        }
                        if api == "anthropic" {
                            assert_eq!(request["thinking"]["budget_tokens"], 1024);
                            assert!(!request.to_string().contains("cache_control"));
                        }
                    }
                    let second = &captured[1];
                    if role != "concierge" {
                        match api {
                            "openai-chat" => {
                                let assistant = second["messages"]
                                    .as_array()
                                    .unwrap()
                                    .iter()
                                    .find(|m| m.get("tool_calls").is_some())
                                    .unwrap();
                                let expected = replay
                                    .unwrap_or(model.to_ascii_lowercase().contains("deepseek"))
                                    && !thinking.is_empty();
                                assert_eq!(
                                    assistant.get("reasoning_content").and_then(Value::as_str),
                                    expected.then_some(thinking)
                                );
                            }
                            "openai-responses" => {
                                assert!(
                                    second["input"]
                                        .as_array()
                                        .unwrap()
                                        .iter()
                                        .any(|i| i["type"] == "function_call_output")
                                );
                                assert!(!second.to_string().contains("local reasoning"));
                            }
                            _ => {
                                let assistant = second["messages"]
                                    .as_array()
                                    .unwrap()
                                    .iter()
                                    .find(|m| m["role"] == "assistant")
                                    .unwrap();
                                let blocks = assistant["content"].as_array().unwrap();
                                assert_eq!(
                                    blocks.iter().any(|b| b["type"] == "thinking"
                                        && b["thinking"] == thinking
                                        && b["signature"] == "local-signature"),
                                    !thinking.is_empty()
                                );
                            }
                        }
                    }
                }
                server.abort();
                drop(db);
                std::fs::remove_dir_all(root).unwrap();
            }
        }
    }
}
