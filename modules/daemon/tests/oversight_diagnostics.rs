use axum::{Json, Router, http::StatusCode, response::IntoResponse, routing::post};
use metteur_daemon::{
    execution::{CheckpointSink, DbCheckpointSink, ExecutionCheckpoint},
    llm::LlmClientFactory,
    oversight::{
        budget,
        diagnostic::{Category, Stage},
        requests, review, scheduler,
    },
    storage::persistence::Db,
};
use metteur_shared::config::{Config, LlmModelConfig, oversight::OversightConfig};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use uuid::Uuid;

#[tokio::test]
async fn real_http_failure_categories_are_safe_correlated_and_survive_reopen() {
    const SECRET: &str = "RAW_PROVIDER_SECRET_AND_PRIVATE_CONTEXT";
    for (case, category, stage) in [
        ("http", Category::ProviderHttp, Stage::Provider),
        ("transport", Category::ProviderTransport, Stage::Provider),
        ("response", Category::ProviderResponse, Stage::Provider),
        ("tool", Category::ToolName, Stage::ToolDispatch),
        ("args", Category::ToolArguments, Stage::ToolDispatch),
        ("final", Category::StructuredFinal, Stage::StructuredFinal),
        ("empty_final", Category::StructuredFinal, Stage::StructuredFinal),
        ("iterations", Category::IterationLimit, Stage::Iteration),
        ("budget", Category::Budget, Stage::Budget),
        ("timeout", Category::Timeout, Stage::Provider),
    ] {
        let requests_seen = Arc::new(AtomicUsize::new(0));
        let seen = requests_seen.clone();
        let app = Router::new().route("/chat/completions", post(move || {
            seen.fetch_add(1, Ordering::SeqCst);
            async move {
                if case == "timeout" { tokio::time::sleep(std::time::Duration::from_millis(200)).await; }
                if case == "http" { return (StatusCode::SERVICE_UNAVAILABLE, SECRET).into_response(); }
                if case == "response" { return Json(json!({"private":SECRET})).into_response(); }
                let message = if matches!(case, "tool" | "args" | "iterations") {
                    json!({"content":"","tool_calls":[{"id":"fixture-call","type":"function","function":{
                        "name": if case == "tool" { "Shell" } else { "ReadRunStats" },
                        "arguments": if case == "args" { json!({"forged":SECRET}).to_string() } else { "{}".into() }
                    }}]})
                } else {
                    json!({"content":if case == "empty_final" { r#"{"verdict":"ok","summary":""}"# } else { SECRET }})
                };
                Json(json!({"choices":[{"message":message}],"usage":{"prompt_tokens":100,"completion_tokens":10}})).into_response()
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = if case == "transport" {
            drop(listener);
            None
        } else {
            Some(tokio::spawn(async move { axum::serve(listener, app).await.unwrap() }))
        };
        let root = std::env::temp_dir().join(format!("diagnostics-{}", Uuid::new_v4()));
        let db = Db::open(&root.join(".metteur/db")).unwrap();
        let run = Uuid::new_v4();
        DbCheckpointSink::new(db.clone(), run)
            .write(&ExecutionCheckpoint::running(run, Uuid::new_v4(), 0))
            .unwrap();
        let mut config = Config::default();
        config.llm.models.insert(
            "fixture".into(),
            LlmModelConfig {
                api_type: "openai-chat".into(),
                api_endpoint: endpoint,
                api_key: "synthetic".into(),
                model_id: "local".into(),
                ..Default::default()
            },
        );
        let mut settings = json!({"model":"fixture","max_review_iterations":1});
        if case == "budget" {
            settings["run_token_budget"] = json!(0);
        }
        if case == "timeout" {
            settings["review_timeout_ms"] = json!(25);
        }
        config.extra.insert("oversight".into(), settings);
        scheduler::initialize(&db, run, OversightConfig::from_config(&config).unwrap()).unwrap();
        requests::receive(
            &db,
            run,
            run,
            Uuid::new_v4(),
            "Inspect local progress",
            requests::Intent {
                category: requests::Category::Query,
                note: "Inspect progress".into(),
            },
        )
        .unwrap();
        let pending = scheduler::claim(&db, run, 1).unwrap().unwrap();
        let (key, client) = review::client(&config, &LlmClientFactory::new()).unwrap();
        let result = review::evaluate(&db, &pending, &config, &key, client.as_ref()).await.unwrap();
        let detail = result.diagnostic.as_ref().unwrap();
        assert_eq!(detail.category, category, "{case}");
        assert_eq!(detail.stage, stage, "{case}");
        assert_eq!((detail.run_id, detail.review_id), (run, pending.review_id));
        assert!(detail.proposal_id.is_none());
        assert_ne!(result.status, scheduler::Status::Completed);
        assert!(result.verdict.is_none());
        assert!(result.actual_action_refs.is_empty());
        let ledger = budget::load(&db, run).unwrap();
        if case == "budget" {
            assert!(ledger.calls.is_empty());
            assert!(detail.call_id.is_none());
            assert_eq!(requests_seen.load(Ordering::SeqCst), 0);
        } else {
            assert_eq!(ledger.calls.len(), 1);
            assert_eq!(detail.call_id, Some(ledger.calls[0].id));
        }
        if matches!(case, "http" | "transport" | "response" | "timeout") {
            assert!(ledger.calls[0].usage.is_none());
            assert!(ledger.calls[0].cost_micros.is_none());
        }
        assert_eq!(detail.http_status, (case == "http").then_some(503));
        let encoded = serde_json::to_string(&result).unwrap();
        assert!(!encoded.contains(SECRET));
        assert!(!encoded.contains("synthetic"));
        assert!(serde_json::to_string(detail).unwrap().len() < 700);
        drop(db);
        let reopened = Db::open(&root.join(".metteur/db")).unwrap();
        let restored = scheduler::load(&reopened, run).unwrap().unwrap().reviews.remove(0);
        assert_eq!(serde_json::to_string(&restored).unwrap(), encoded);
        assert_eq!(
            requests::load(&reopened, run).unwrap().requests[0].state,
            requests::State::Failed
        );
        drop(reopened);
        if case == "http" {
            let output = read_with_cli(&root, run).await;
            assert!(output.contains("provider_http"));
            assert!(output.contains(&pending.review_id.to_string()));
            assert!(output.contains(&detail.call_id.unwrap().to_string()));
            assert!(!output.contains(SECRET));
        }
        if let Some(server) = server {
            server.abort();
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}

async fn read_with_cli(root: &std::path::Path, run: Uuid) -> String {
    use metteur_cli::commands::{self, Outcome, SessionState};
    use metteur_daemon::{grpc::{AppState, DaemonService}, registry::Registry, workspace::WorkspaceManager};
    use metteur_proto::proto::{daemon_client::DaemonClient, daemon_server::DaemonServer};
    let app = Arc::new(AppState::new(WorkspaceManager::new().with_global_config_path(root.join("global.toml")), Arc::new(Registry::with_builtins()), Default::default()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        tonic::transport::Server::builder().add_service(DaemonServer::new(DaemonService::new(app)))
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener)).await.unwrap();
    });
    let mut client = DaemonClient::connect(endpoint).await.unwrap();
    let mut state = SessionState::default();
    commands::dispatch(&mut client, &mut state, commands::parse(&format!("open {}", root.display())).unwrap()).await.unwrap();
    let Outcome::Printed(output) = commands::dispatch(&mut client, &mut state, commands::parse(&format!("reviews {run}")).unwrap()).await.unwrap() else { panic!("expected report") };
    commands::dispatch(&mut client, &mut state, commands::parse(&format!("close {}", root.display())).unwrap()).await.unwrap();
    server.abort();
    let _ = server.await;
    output
}
