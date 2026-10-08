use metteur_daemon::{
    execution::{
        context::ExecutionContext,
        react::{ReactOptions, run_react},
    },
    llm::{LlmClientFactory, LlmProviderConfig, ProviderKind, billing},
    observability::audit::AuditWriter,
    oversight::budget::{self, Caller},
    registry::Registry,
    storage::persistence::Db,
};
use metteur_shared::{
    config::{Config, LlmModelConfig, ModelPricing, ModelPricingStrategy, PricingTier},
    llm::ContextManager,
};
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

fn model() -> LlmModelConfig {
    LlmModelConfig {
        pricing: ModelPricingStrategy {
            kind: "tiered".into(),
            tiers: vec![
                PricingTier {
                    max_input_tokens: Some(100),
                    prices: ModelPricing {
                        input_per_mtok: 1.0,
                        output_per_mtok: 2.0,
                        cache_hit_per_mtok: 0.1,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                PricingTier {
                    max_input_tokens: None,
                    prices: ModelPricing {
                        input_per_mtok: 2.0,
                        output_per_mtok: 4.0,
                        cache_hit_per_mtok: 0.5,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn input_thresholds_are_inclusive_and_unmatched_or_invalid_prices_are_unknown() {
    let mut model = model();
    for (input, price) in [(0, 1.0), (99, 1.0), (100, 1.0), (101, 2.0), (u64::MAX, 2.0)] {
        assert_eq!(billing::effective_pricing(&model, input).unwrap().input_per_mtok, price);
    }
    model.pricing.tiers[1].max_input_tokens = Some(200);
    assert!(billing::effective_pricing(&model, 201).is_none());
    assert!(billing::effective_pricing(&model, 200).is_some());
    model.pricing.tiers[1].max_input_tokens = Some(100);
    assert!(billing::effective_pricing(&model, 50).is_none());
    model.pricing.kind = "default".into();
    model.pricing.tiers.clear();
    model.pricing.prices = Some(ModelPricing {
        input_per_mtok: 3.0,
        ..Default::default()
    });
    assert_eq!(billing::effective_pricing(&model, u64::MAX).unwrap().input_per_mtok, 3.0);
}

#[tokio::test]
async fn main_and_oversight_http_calls_price_each_request_before_aggregation() {
    use axum::{Json, Router, routing::post};
    let inputs = Arc::new(std::sync::Mutex::new(std::collections::VecDeque::from([
        99, 99, 100, 100, 101, 101,
    ])));
    let queue = inputs.clone();
    let app = Router::new().route("/chat/completions", post(move || {
        let input = queue.lock().unwrap().pop_front().unwrap();
        async move { Json(json!({"choices":[{"message":{"content":"ok"}}],"usage":{"prompt_tokens":input,"completion_tokens":10,"completion_tokens_details":{"reasoning_tokens":5},"prompt_tokens_details":{"cached_tokens":80}}})) }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let root = std::env::temp_dir().join(format!("tiered-{}", Uuid::new_v4()));
    let db = Db::open(&root.join("db")).unwrap();
    let main_run = Uuid::new_v4();
    let oversight_run = Uuid::new_v4();
    let mut config = Config::default();
    let mut model = model();
    model.api_type = "openai-chat".into();
    model.api_endpoint = endpoint.clone();
    model.api_key = "synthetic".into();
    model.model_id = "provider-model".into();
    config.llm.models.insert("priced".into(), model);
    let mut ctx =
        ExecutionContext::new(Arc::new(Registry::with_builtins()), LlmClientFactory::new(), root)
            .with_run(main_run, 1);
    ctx.config = Some(Arc::new(tokio::sync::RwLock::new(config.clone())));
    ctx.audit = Some(AuditWriter::new(db.clone()));
    let client = LlmClientFactory::new()
        .create(&LlmProviderConfig::new(
            ProviderKind::OpenAiChat,
            endpoint,
            "synthetic",
            "provider-model",
        ))
        .unwrap();
    for _ in 0..3 {
        run_react(
            &mut ctx,
            ContextManager::new_from_prompt(vec![], "test"),
            &ReactOptions {
                model: Some("priced".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        budget::complete(
            &db,
            oversight_run,
            Caller::Supervisor,
            "priced",
            &config,
            client.as_ref(),
            &ContextManager::new_from_prompt(vec![], "test"),
        )
        .await
        .unwrap();
    }
    for run in [main_run, oversight_run] {
        let summary = billing::run_usage(&db, &config.billing, &run.to_string()).unwrap();
        assert_eq!(summary.total_cost_micros, 217);
        assert_eq!(summary.models[0].calls, 3);
        assert!(summary.models[0].cost_complete);
        assert_eq!(summary.models[0].input_tokens, 300);
        assert_eq!(summary.models[0].cached_input_tokens, 240);
    }
    assert_eq!(
        budget::load(&db, oversight_run)
            .unwrap()
            .calls
            .iter()
            .map(|call| call.cost_micros.unwrap())
            .collect::<Vec<_>>(),
        vec![47, 48, 122]
    );
    assert!(inputs.lock().unwrap().is_empty());
    server.abort();
}
