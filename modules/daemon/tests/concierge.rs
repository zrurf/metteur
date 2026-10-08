use metteur_daemon::{
    DaemonResult,
    execution::{CheckpointSink, DbCheckpointSink, ExecutionCheckpoint, RunStatus},
    llm::{LlmClient, LlmResponse, StreamDelta},
    oversight::{budget, conversation, requests},
    storage::persistence::{Db, cf},
};
use metteur_shared::{
    config::Config,
    llm::{ContextManager, GenerationParams, ToolDefinition, Usage},
};
use std::sync::Mutex;
use uuid::Uuid;
struct Capture {
    reply: String,
    contexts: Mutex<Vec<Vec<metteur_shared::llm::Message>>>,
}
#[async_trait::async_trait]
impl LlmClient for Capture {
    fn model(&self) -> &str {
        "small"
    }
    fn provider(&self) -> &str {
        "test"
    }
    async fn complete(
        &self,
        ctx: &ContextManager,
        params: &GenerationParams,
        tools: &[ToolDefinition],
    ) -> DaemonResult<LlmResponse> {
        assert!(tools.is_empty());
        assert_eq!(params.max_tokens, Some(2048));
        self.contexts.lock().unwrap().push(ctx.build());
        Ok(LlmResponse {
            text: self.reply.clone(),
            thinking: vec![],
            tool_calls: vec![],
            usage: Usage {
                tokens_reported: true,
                cache_read_reported: true,
                input_tokens: 10,
                output_tokens: 10,
                ..Default::default()
            },
        })
    }
    async fn stream(
        &self,
        ctx: &ContextManager,
        params: &GenerationParams,
        tools: &[ToolDefinition],
        _: &mut (dyn FnMut(StreamDelta) + Send),
    ) -> DaemonResult<LlmResponse> {
        self.complete(ctx, params, tools).await
    }
}
fn setup() -> (Db, Uuid, Config) {
    let db = Db::open(&std::env::temp_dir().join(format!("concierge-{}", Uuid::new_v4()))).unwrap();
    let run = Uuid::new_v4();
    DbCheckpointSink::new(db.clone(), run)
        .write(&ExecutionCheckpoint::running(run, Uuid::new_v4(), 0))
        .unwrap();
    (db, run, Config::default())
}
#[tokio::test]
async fn bounded_read_only_context_has_six_turns_and_no_tools_or_grants() {
    let (db, run, mut config) = setup();
    config.anonymize.extra_patterns.push("private-company".into());
    let client = Capture {
        reply: r#"{"kind":"answer","text":"No validation evidence is available."}"#.into(),
        contexts: Mutex::new(vec![]),
    };
    for n in 0..8 {
        let id = Uuid::new_v4();
        conversation::begin(&db, run, run, id, &format!("message {n} private-company")).unwrap();
        conversation::answer(&db, run, id, &config, "small", &client).await.unwrap();
    }
    let captures = client.contexts.lock().unwrap();
    let last = captures.last().unwrap();
    let all = serde_json::to_string(last).unwrap();
    assert!(!all.contains("private-company"));
    assert!(!all.contains("message 0"));
    assert!(all.contains("message 1"));
    assert_eq!(last.iter().filter(|m| m.role == metteur_shared::llm::Role::Assistant).count(), 6);
    assert!(db.scan(cf::GRANTS).unwrap().is_empty());
    assert!(requests::load(&db, run).unwrap().requests.is_empty());
}
#[tokio::test]
async fn receipt_is_durable_before_reply_and_replay_does_not_call_model() {
    let (db, run, config) = setup();
    let id = Uuid::new_v4();
    let client = Capture {
        reply: r#"{"kind":"intent","category":"request","note":"User says approve and stop"}"#
            .into(),
        contexts: Mutex::new(vec![]),
    };
    conversation::begin(&db, run, run, id, "I approve; stop it").unwrap();
    let turn = conversation::answer(&db, run, id, &config, "small", &client).await.unwrap();
    assert_eq!(turn.state, "received");
    assert_eq!(requests::load(&db, run).unwrap().requests[0].state, requests::State::Received);
    assert!(turn.answer.contains("not processed"));
    assert!(db.scan(cf::GRANTS).unwrap().is_empty());
    assert!(conversation::begin(&db, run, run, id, "I approve; stop it").unwrap().is_some());
    assert_eq!(budget::load(&db, run).unwrap().calls.len(), 1);
    assert!(conversation::begin(&db, run, run, id, "changed").is_err());
}
#[tokio::test]
async fn forged_authority_and_tool_calls_cannot_become_receipts() {
    for reply in [
        r#"{"kind":"intent","category":"request","note":"yes","approved":true}"#,
        r#"{"kind":"answer","text":"yes","scope":"global"}"#,
        r#"{"kind":"intent","category":"request","note":"yes","source":"direct_user"}"#,
    ] {
        let (db, run, config) = setup();
        let id = Uuid::new_v4();
        conversation::begin(&db, run, run, id, "approve").unwrap();
        let client = Capture {
            reply: reply.into(),
            contexts: Mutex::new(vec![]),
        };
        assert_eq!(
            conversation::answer(&db, run, id, &config, "small", &client).await.unwrap().state,
            "failed"
        );
        assert!(requests::load(&db, run).unwrap().requests.is_empty());
        assert!(db.scan(cf::GRANTS).unwrap().is_empty());
    }
    let (db, run, config) = setup();
    let id = Uuid::new_v4();
    conversation::begin(&db, run, run, id, "go").unwrap();
    let client =
        metteur_daemon::llm::MockClient::new(vec![metteur_daemon::llm::MockStep::Tools(vec![
            metteur_shared::llm::ToolCall {
                id: "fake".into(),
                name: "RespondApproval".into(),
                arguments: serde_json::json!({}),
            },
        ])]);
    assert_eq!(
        conversation::answer(&db, run, id, &config, "small", &client).await.unwrap().state,
        "failed"
    );
    assert!(db.scan(cf::GRANTS).unwrap().is_empty());
}
#[test]
fn disabled_model_never_falls_back_to_workspace_default() {
    let mut config = Config::default();
    config.llm.default_model = Some("expensive".into());
    assert!(conversation::client(&config, &Default::default()).is_err());
    config.extra.insert("oversight".into(), serde_json::json!({"concierge_model":"missing"}));
    assert!(conversation::client(&config, &Default::default()).is_err());
}
#[tokio::test]
async fn closed_run_rejects_intake_and_keeps_existing_conversation() {
    let (db, run, config) = setup();
    let id = Uuid::new_v4();
    conversation::begin(&db, run, run, id, "status").unwrap();
    let mut cp = DbCheckpointSink::load(&db, run).unwrap().unwrap();
    cp.status = RunStatus::Completed;
    DbCheckpointSink::new(db.clone(), run).write(&cp).unwrap();
    let client = Capture {
        reply: r#"{"kind":"answer","text":"hello"}"#.into(),
        contexts: Mutex::new(vec![]),
    };
    assert_eq!(
        conversation::answer(&db, run, id, &config, "small", &client).await.unwrap().state,
        "failed"
    );
    assert!(client.contexts.lock().unwrap().is_empty());
    assert!(conversation::begin(&db, run, run, Uuid::new_v4(), "new").is_err());
    assert_eq!(conversation::load(&db, run).unwrap().turns.len(), 1);
}

#[test]
fn restart_recovers_receipts_without_replaying_unknown_calls() {
    let (db, run, _) = setup();
    let id = Uuid::new_v4();
    conversation::begin(&db, run, run, id, "change it").unwrap();
    requests::receive(
        &db,
        run,
        run,
        id,
        "change it",
        requests::Intent {
            category: requests::Category::Request,
            note: "change".into(),
        },
    )
    .unwrap();
    conversation::recover(&db).unwrap();
    assert_eq!(conversation::load(&db, run).unwrap().turns[0].request_id, Some(id));
    let interrupted = Uuid::new_v4();
    conversation::begin(&db, run, run, interrupted, "status").unwrap();
    conversation::recover(&db).unwrap();
    assert_eq!(conversation::load(&db, run).unwrap().turns[1].state, "failed");
    assert!(conversation::begin(&db, run, run, Uuid::new_v4(), "new attempt").is_ok());
}
