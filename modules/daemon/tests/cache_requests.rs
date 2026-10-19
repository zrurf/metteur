//! HC-01/02/04: inspect complete serialized HTTP requests, never a paid provider.
use axum::{Json, Router, response::IntoResponse, routing::post};
use metteur_daemon::llm::{LlmClientFactory, LlmProviderConfig, ProviderKind};
use metteur_shared::llm::{
    ContentBlock, ContextManager, GenerationParams, Message, Role, SystemFragment, ToolCall,
    ToolDefinition,
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

fn markers(value: &Value) -> usize {
    match value {
        Value::Object(fields) => {
            usize::from(fields.contains_key("cache_control"))
                + fields.values().map(markers).sum::<usize>()
        }
        Value::Array(items) => items.iter().map(markers).sum(),
        _ => 0,
    }
}

fn fragments(count: usize) -> Vec<SystemFragment> {
    let mut fragments: Vec<_> = (0..count)
        .map(|i| SystemFragment {
            priority: 100 - i as i32,
            scope: format!("stable-{i:03}"),
            content: format!("stable section {i}: {}", "local evidence ".repeat(30)),
        })
        .collect();
    if count > 0 {
        fragments.push(SystemFragment {
            priority: -10,
            scope: "harness.env.date".into(),
            content: "date: day one".into(),
        });
        fragments.push(SystemFragment {
            priority: 200,
            scope: "empty".into(),
            content: "".into(),
        });
        fragments.push(SystemFragment {
            priority: 200,
            scope: "blank".into(),
            content: " \n\t".into(),
        });
        // Node/addon fragments can sort after the date; the stable breakpoint
        // must precede the date even when it is not the last system block.
        fragments.push(SystemFragment {
            priority: -20,
            scope: "node.append".into(),
            content: "additional node context".into(),
        });
        for content in ["tie beta", "tie alpha"] {
            fragments.push(SystemFragment {
                priority: 50,
                scope: "same-scope".into(),
                content: content.into(),
            });
        }
    }
    fragments.reverse();
    fragments
}

fn tools() -> Vec<ToolDefinition> {
    ["Zulu", "Alpha"]
        .iter()
        .map(|name| ToolDefinition {
            name: (*name).into(),
            description: format!("Local {name}"),
            parameters: json!({"type":"object","properties":{}}),
        })
        .collect()
}

#[tokio::test]
async fn final_http_requests_share_one_breakpoint_budget_and_canonical_prefix() {
    let captured = Arc::new(Mutex::new(Vec::<Value>::new()));
    let requests = captured.clone();
    let app = Router::new().route(
        "/messages",
        post(move |Json(body): Json<Value>| {
            let streaming = body["stream"] == true;
            requests.lock().unwrap().push(body);
            async move {
                let usage = json!({"input_tokens":1,"output_tokens":1});
                if streaming {
                    let events = [
                        json!({"type":"message_start","message":{"usage":usage}}),
                        json!({"type":"message_delta","usage":{"output_tokens":1}}),
                    ];
                    (
                        [("content-type", "text/event-stream")],
                        events.iter().map(|e| format!("data: {e}\n\n")).collect::<String>(),
                    )
                        .into_response()
                } else {
                    Json(json!({"content":[],"usage":usage})).into_response()
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    for cache in [true, false] {
        for count in [8, 0, 1, 2, 48] {
            for with_tools in [true, false] {
                let client = LlmClientFactory::new()
                    .create(
                        &LlmProviderConfig::new(
                            ProviderKind::Anthropic,
                            &endpoint,
                            "synthetic",
                            "local",
                        )
                        .with_prompt_cache(cache),
                    )
                    .unwrap();
                let mut context =
                    ContextManager::new_from_prompt(fragments(count), "first user turn");
                for i in 0..30 {
                    context.messages.push(Message::text(
                        if i % 2 == 0 {
                            Role::Assistant
                        } else {
                            Role::User
                        },
                        format!("history {i}"),
                    ));
                }
                context.messages.push(Message {
                    role: Role::Assistant,
                    content: vec![
                        ContentBlock::Thinking {
                            text: "reasoning".into(),
                            signature: Some("unchanged-signature".into()),
                        },
                        ContentBlock::RedactedThinking {
                            data: "opaque".into(),
                        },
                    ],
                    tool_calls: vec![ToolCall {
                        id: "local-call".into(),
                        name: "Alpha".into(),
                        arguments: json!({}),
                    }],
                    tool_call_id: None,
                });
                context.messages.push(Message {
                    role: Role::Tool,
                    content: vec![ContentBlock::Text("actual local result".into())],
                    tool_calls: vec![],
                    tool_call_id: Some("local-call".into()),
                });
                let mut definitions = if with_tools {
                    tools()
                } else {
                    vec![]
                };
                client
                    .complete(&context, &GenerationParams::default(), &definitions)
                    .await
                    .unwrap();
                let first = captured.lock().unwrap().last().unwrap().clone();
                assert!(
                    markers(&first)
                        <= if cache {
                            4
                        } else {
                            0
                        },
                    "cache={cache}, fragments={count}, tools={with_tools}: {} markers",
                    markers(&first)
                );
                if cache && count > 0 {
                    let blocks = first["system"].as_array().unwrap();
                    let expected: Vec<_> = context
                        .ordered_fragments()
                        .into_iter()
                        .filter(|f| !f.content.trim().is_empty())
                        .map(|f| f.content.as_str())
                        .collect();
                    assert_eq!(
                        blocks.iter().map(|b| b["text"].as_str().unwrap()).collect::<Vec<_>>(),
                        expected
                    );
                    assert_eq!(
                        expected.join("\n\n"),
                        metteur_harness::assembly::render_all(
                            context
                                .system_fragments
                                .iter()
                                .filter(|f| !f.content.trim().is_empty())
                        )
                    );
                    assert!(blocks.iter().all(|b| !b["text"].as_str().unwrap().is_empty()));
                    let date = blocks.iter().position(|b| b["text"] == "date: day one").unwrap();
                    assert!(
                        blocks[date - 1].get("cache_control").is_some(),
                        "stable prefix needs its own breakpoint before the date"
                    );
                }
                if with_tools {
                    assert_eq!(first["tools"][0]["name"], "Alpha");
                }
                let messages = first["messages"].as_array().unwrap();
                let previous = &messages[messages.len() - 2]["content"];
                assert_eq!(previous[0]["signature"], "unchanged-signature");
                assert_eq!(previous[1]["data"], "opaque");
                assert!(previous[0].get("cache_control").is_none());
                assert!(previous[1].get("cache_control").is_none());
                assert_eq!(messages.last().unwrap()["content"][0]["tool_use_id"], "local-call");

                context.system_fragments.reverse();
                definitions.reverse();
                client
                    .stream(&context, &GenerationParams::default(), &definitions, &mut |_| {})
                    .await
                    .unwrap();
                let mut second = captured.lock().unwrap().last().unwrap().clone();
                second.as_object_mut().unwrap().remove("stream");
                assert_eq!(
                    first, second,
                    "streaming and input order must not change the serialized prefix"
                );
                if count > 0 {
                    context
                        .system_fragments
                        .iter_mut()
                        .find(|f| f.scope == "harness.env.date")
                        .unwrap()
                        .content = "date: day two".into();
                    client
                        .complete(&context, &GenerationParams::default(), &definitions)
                        .await
                        .unwrap();
                    let changed = captured.lock().unwrap().last().unwrap().clone();
                    if cache {
                        let before = first["system"].as_array().unwrap();
                        let after = changed["system"].as_array().unwrap();
                        let date =
                            before.iter().position(|b| b["text"] == "date: day one").unwrap();
                        assert_eq!(&before[..date], &after[..date]);
                        assert_ne!(before[date], after[date]);
                        assert_eq!(&before[date + 1..], &after[date + 1..]);
                    }
                }
            }
        }
    }
    server.abort();
}

#[tokio::test]
async fn empty_system_and_thinking_only_tail_do_not_get_invalid_markers() {
    let captured = Arc::new(Mutex::new(Vec::<Value>::new()));
    let requests = captured.clone();
    let app = Router::new().route(
        "/messages",
        post(move |Json(body): Json<Value>| {
            requests.lock().unwrap().push(body);
            async { Json(json!({"content":[],"usage":{"input_tokens":1,"output_tokens":1}})) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = LlmClientFactory::new()
        .create(
            &LlmProviderConfig::new(
                ProviderKind::Anthropic,
                format!("http://{}", listener.local_addr().unwrap()),
                "synthetic",
                "local",
            )
            .with_prompt_cache(true),
        )
        .unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut context = ContextManager::new_from_prompt(
        vec![SystemFragment {
            priority: 0,
            scope: "empty".into(),
            content: "".into(),
        }],
        "",
    );
    context.messages = vec![Message {
        role: Role::Assistant,
        content: vec![ContentBlock::Thinking {
            text: "reasoning".into(),
            signature: Some("signature".into()),
        }],
        tool_calls: vec![],
        tool_call_id: None,
    }];
    client.complete(&context, &GenerationParams::default(), &[]).await.unwrap();
    let body = captured.lock().unwrap().last().unwrap().clone();
    assert!(body.get("system").is_none());
    assert_eq!(markers(&body), 0);
    context.messages = vec![Message::text(Role::User, "")];
    client.complete(&context, &GenerationParams::default(), &[]).await.unwrap();
    assert_eq!(markers(captured.lock().unwrap().last().unwrap()), 0);
    server.abort();
}
