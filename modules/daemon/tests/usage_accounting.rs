use axum::{Json, Router, response::IntoResponse, routing::post};
use metteur_daemon::{
    llm::{LlmClientFactory, LlmProviderConfig, ProviderKind, billing},
    oversight::budget::{self, Caller},
    storage::persistence::{Db, cf},
};
use metteur_shared::{
    Usage,
    config::{Config, LlmModelConfig, ModelPricing, oversight::OversightConfig},
    llm::{ContextManager, GenerationParams},
};
use serde_json::{Value, json};
use uuid::Uuid;

#[tokio::test]
async fn http_providers_share_streaming_and_nonstreaming_accounting() {
    let mut aggregate = Usage {
        tokens_reported: true,
        cache_read_reported: true,
        ..Default::default()
    };
    for (kind, path) in [
        (ProviderKind::OpenAiChat, "/chat/completions"),
        (ProviderKind::OpenAiResponses, "/responses"),
        (ProviderKind::Anthropic, "/messages"),
    ] {
        for reasoning in [None, Some(0), Some(5)] {
            let mut raw = match kind {
                ProviderKind::OpenAiChat => {
                    json!({"prompt_tokens":100,"completion_tokens":10,"prompt_tokens_details":{"cached_tokens":80}})
                }
                ProviderKind::OpenAiResponses => {
                    json!({"input_tokens":100,"output_tokens":10,"input_tokens_details":{"cached_tokens":80}})
                }
                ProviderKind::Anthropic => {
                    json!({"input_tokens":10,"output_tokens":10,"cache_read_input_tokens":80,"cache_creation_input_tokens":10})
                }
            };
            if let Some(n) = reasoning {
                match kind {
                    ProviderKind::OpenAiChat => {
                        raw["completion_tokens_details"] = json!({"reasoning_tokens":n})
                    }
                    ProviderKind::OpenAiResponses => {
                        raw["output_tokens_details"] = json!({"reasoning_tokens":n})
                    }
                    ProviderKind::Anthropic => {}
                }
            }
            let app = Router::new().route(
                path,
                post(move |Json(request): Json<Value>| {
                    let raw = raw.clone();
                    async move {
                        if request["stream"] == true {
                            let events = match kind {
                                ProviderKind::OpenAiChat => vec![json!({"usage":raw})],
                                ProviderKind::OpenAiResponses => vec![
                                    json!({"type":"response.completed","response":{"usage":raw}}),
                                ],
                                ProviderKind::Anthropic => vec![
                                    json!({"type":"message_start","message":{"usage":raw}}),
                                    json!({"type":"message_delta","usage":{"output_tokens":10}}),
                                ],
                            };
                            (
                                [("content-type", "text/event-stream")],
                                events.iter().map(|e| format!("data: {e}\n\n")).collect::<String>(),
                            )
                                .into_response()
                        } else {
                            let result = match kind {
                                ProviderKind::OpenAiChat => {
                                    json!({"choices":[{"message":{"content":"ok"}}],"usage":raw})
                                }
                                ProviderKind::OpenAiResponses => json!({"output":[],"usage":raw}),
                                ProviderKind::Anthropic => json!({"content":[],"usage":raw}),
                            };
                            Json(result).into_response()
                        }
                    }
                }),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let client = LlmClientFactory::new()
                .create(&LlmProviderConfig::new(
                    kind,
                    format!("http://{addr}"),
                    "synthetic",
                    "test",
                ))
                .unwrap();
            let context = ContextManager::new_from_prompt(vec![], "test");
            let params = GenerationParams::default();
            let plain = client.complete(&context, &params, &[]).await.unwrap().usage;
            let streamed = client.stream(&context, &params, &[], &mut |_| {}).await.unwrap().usage;
            assert_eq!(plain, streamed);
            assert!(plain.tokens_reported);
            assert_eq!(
                (plain.input_tokens, plain.output_tokens, plain.total_tokens),
                (100, 10, 110)
            );
            assert_eq!(
                plain.reasoning_tokens,
                if kind == ProviderKind::Anthropic {
                    0
                } else {
                    reasoning.unwrap_or(0)
                }
            );
            assert_eq!(plain.cache_hit_rate(), Some(0.8));
            let p = ModelPricing {
                input_per_mtok: 1.0,
                output_per_mtok: 2.0,
                ..Default::default()
            };
            assert_eq!(billing::cost(Some(&p), "USD", &plain).unwrap().micros, 120);
            aggregate.add(&plain);
            if kind == ProviderKind::OpenAiChat && reasoning == Some(5) {
                use metteur_daemon::execution::{
                    context::ExecutionContext,
                    react::{ReactOptions, run_react},
                };
                use std::sync::{Arc, atomic::Ordering};
                let root = std::env::temp_dir().join(format!("metrics-{}", Uuid::new_v4()));
                std::fs::create_dir_all(&root).unwrap();
                let mut ctx = ExecutionContext::new(
                    Arc::new(metteur_daemon::registry::Registry::with_builtins()),
                    LlmClientFactory::new(),
                    root,
                );
                let metrics = Arc::new(metteur_daemon::observability::metrics::Metrics::default());
                ctx.metrics = Some(metrics.clone());
                let options = ReactOptions {
                    provider: "openai-chat".into(),
                    model: Some("test".into()),
                    base_url: Some(format!("http://{addr}")),
                    api_key: Some("synthetic".into()),
                    ..Default::default()
                };
                run_react(&mut ctx, ContextManager::new_from_prompt(vec![], "test"), &options)
                    .await
                    .unwrap();
                assert_eq!(metrics.llm_output_tokens_total.load(Ordering::Relaxed), 10);
                assert_eq!(metrics.llm_input_tokens_total.load(Ordering::Relaxed), 100);
            }
            server.abort();
        }
    }
    assert_eq!(
        (aggregate.input_tokens, aggregate.output_tokens, aggregate.total_tokens),
        (900, 90, 990)
    );
    assert!(billing::cost(Some(&ModelPricing::default()), "USD", &Usage::default()).is_none());
}

#[test]
fn exact_budget_boundary_restart_and_legacy_records_do_not_refund() {
    let path = std::env::temp_dir().join(format!("accounting-{}", Uuid::new_v4()));
    let db = Db::open(&path).unwrap();
    let run = Uuid::new_v4();
    let mut config = Config::default();
    let mut model = LlmModelConfig::default();
    model.pricing.prices = Some(ModelPricing {
        input_per_mtok: 1.0,
        output_per_mtok: 2.0,
        ..Default::default()
    });
    config.llm.models.insert("test".into(), model);
    let settings = OversightConfig {
        run_token_budget: 110,
        ..Default::default()
    };
    let id = budget::reserve(&db, run, Caller::Supervisor, "test", 110, &settings).unwrap();
    let usage = Usage {
        tokens_reported: true,
        cache_read_reported: true,
        input_tokens: 100,
        output_tokens: 10,
        reasoning_tokens: 5,
        total_tokens: 110,
        ..Default::default()
    };
    budget::settle(&db, run, id, Some(usage), "test", &config).unwrap();
    let summary = budget::summary(&db, run, &settings).unwrap();
    assert_eq!(summary.charged, 110);
    assert!(summary.exhausted);
    assert_eq!(summary.calls[0].cost_micros, Some(120));
    assert!(budget::reserve(&db, run, Caller::Concierge, "test", 1, &settings).is_err());
    let audit = metteur_daemon::observability::audit::AuditWriter::new(db.clone());
    audit.record("test", "llm.usage", json!({"run_id":"legacy","model":"test","tokens_reported":true,"input_tokens":100,"output_tokens":10,"reasoning_tokens":5,"cost_micros":130,"currency":"USD"})).unwrap();
    let before = audit.list().unwrap();
    let historical = billing::run_usage(&db, &config.billing, "legacy").unwrap();
    assert!(historical.models[0].tokens_complete);
    assert!(!historical.models[0].cost_complete);
    assert_eq!(historical.models[0].output_tokens, 10);
    assert_eq!(historical.total_cost_micros, 130);
    assert_eq!(
        serde_json::to_value(audit.list().unwrap()).unwrap(),
        serde_json::to_value(before).unwrap()
    );
    drop(audit);
    let mut legacy = serde_json::to_value(budget::load(&db, run).unwrap()).unwrap();
    legacy["calls"][0].as_object_mut().unwrap().remove("accounting_version");
    legacy["calls"][0]["charged"] = json!(115);
    legacy["calls"][0]["cost_micros"] = json!(130);
    let bytes = serde_json::to_vec(&legacy).unwrap();
    let key = format!("oversight:budget:{run}");
    db.put_durable(cf::EXECUTION_STATE, key.as_bytes(), &bytes).unwrap();
    drop(db);
    let db = Db::open(&path).unwrap();
    let summary = budget::summary(&db, run, &settings).unwrap();
    assert_eq!(summary.charged, 115);
    assert_eq!(summary.calls[0].accounting_version, 0);
    assert!(
        !billing::run_usage(&db, &config.billing, &run.to_string()).unwrap().models[0]
            .cost_complete
    );
    assert_eq!(db.get(cf::EXECUTION_STATE, key.as_bytes()).unwrap().unwrap(), bytes);
    assert!(budget::reserve(&db, run, Caller::Concierge, "test", 1, &settings).is_err());
}
