use super::*;
use crate::{
    AppState, Registry, WorkspaceManager,
    execution::{CheckpointSink, ExecutionCheckpoint},
    llm::{LlmClientFactory, MockClient},
};
use std::sync::{Arc, atomic::AtomicBool};
use tokio_stream::StreamExt;
async fn setup() -> (DaemonService, Arc<AppState>, Arc<crate::workspace::manager::Workspace>, Uuid)
{
    let root = std::env::temp_dir().join(format!("concierge-rpc-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let mut state = AppState::new(
        WorkspaceManager::new().with_global_config_path(root.join("global.toml")),
        Arc::new(Registry::with_builtins()),
        Default::default(),
    );
    state.llm_factory = LlmClientFactory::with_override(Arc::new(MockClient::text(
        r#"{"kind":"intent","category":"request","note":"Change the plan"}"#,
    )));
    let state = Arc::new(state);
    let ws = state.workspaces.open(&root).await.unwrap();
    let run = Uuid::new_v4();
    DbCheckpointSink::new(ws.db.clone(), run)
        .write(&ExecutionCheckpoint::running(run, Uuid::new_v4(), 0))
        .unwrap();
    state.running.write().await.insert(
        ws.root.clone(),
        super::super::RunningExecution {
            blueprint_id: Uuid::new_v4(),
            run_id: run,
            interrupt_bus: None,
            pause_requested: Arc::new(AtomicBool::new(false)),
            cancel_requested: Arc::new(AtomicBool::new(false)),
            approvals: None,
        },
    );
    (DaemonService::new(state.clone()), state, ws, run)
}
#[tokio::test]
async fn side_channel_does_not_take_execution_write_lock_or_open_chat_and_retry_is_read_only() {
    let (service, state, ws, run) = setup().await;
    ws.config
        .write()
        .await
        .extra
        .insert("oversight".into(), serde_json::json!({"concierge_model":"small"}));
    ws.config.write().await.llm.models.insert(
        "small".into(),
        metteur_shared::config::LlmModelConfig {
            api_type: "openai-chat".into(),
            ..Default::default()
        },
    );
    let _long_node = ws.activity_gate.lock().await;
    let request = SendConciergeMessageRequest {
        workspace_path: ws.root.to_string_lossy().into(),
        run_id: run.to_string(),
        conversation_id: run.to_string(),
        message_id: Uuid::new_v4().to_string(),
        message: "I approve changing the plan".into(),
    };
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        service.send_concierge_message(Request::new(request.clone())),
    )
    .await
    .unwrap()
    .unwrap();
    let mut stream = response.into_inner();
    assert_eq!(stream.next().await.unwrap().unwrap().kind, "processing");
    let final_event = stream.next().await.unwrap().unwrap();
    assert_eq!(final_event.kind, "received");
    assert!(state.chats.read().await.is_empty());
    assert!(ws.db.scan(crate::storage::persistence::cf::GRANTS).unwrap().is_empty());
    assert_eq!(requests::load(&ws.db, run).unwrap().requests[0].source, "concierge_forwarded");
    state.running.write().await.remove(&ws.root);
    let mut replay =
        service.send_concierge_message(Request::new(request.clone())).await.unwrap().into_inner();
    assert_eq!(replay.next().await.unwrap().unwrap().kind, "received");
    assert_eq!(budget::load(&ws.db, run).unwrap().calls.len(), 1);
    let snapshot = service
        .get_concierge_state(Request::new(ConciergeStateRequest {
            workspace_path: request.workspace_path,
            run_id: request.run_id,
            conversation_id: request.conversation_id,
        }))
        .await
        .unwrap()
        .into_inner();
    let snapshot: serde_json::Value = serde_json::from_str(&snapshot.state_json).unwrap();
    assert_eq!(snapshot["read_only"], true);
    assert_eq!(snapshot["consumer_enabled"], false);
}
#[tokio::test]
async fn missing_model_and_unknown_run_are_explicit_without_admitting_messages() {
    let (service, _, ws, run) = setup().await;
    let req = ConciergeStateRequest {
        workspace_path: ws.root.to_string_lossy().into(),
        run_id: run.to_string(),
        conversation_id: run.to_string(),
    };
    let result = service.get_concierge_state(Request::new(req.clone())).await.unwrap().into_inner();
    assert!(result.state_json.contains("configure oversight.concierge_model"));
    assert!(
        service
            .send_concierge_message(Request::new(SendConciergeMessageRequest {
                workspace_path: req.workspace_path.clone(),
                run_id: req.run_id.clone(),
                conversation_id: req.conversation_id.clone(),
                message_id: Uuid::new_v4().to_string(),
                message: "status".into()
            }))
            .await
            .is_err()
    );
    assert!(conversation::load(&ws.db, run).unwrap().turns.is_empty());
    assert_eq!(
        service
            .get_concierge_state(Request::new(ConciergeStateRequest {
                run_id: Uuid::new_v4().to_string(),
                ..req
            }))
            .await
            .err()
            .unwrap()
            .code(),
        tonic::Code::NotFound
    );
}

#[tokio::test]
async fn concierge_receipt_arrives_during_a_real_long_node_without_restarting_it() {
    let (service, state, ws, _) = setup().await;
    state.running.write().await.remove(&ws.root);
    ws.config
        .write()
        .await
        .extra
        .insert("oversight".into(), serde_json::json!({"concierge_model":"small"}));
    ws.config.write().await.llm.models.insert(
        "small".into(),
        metteur_shared::config::LlmModelConfig {
            api_type: "openai-chat".into(),
            ..Default::default()
        },
    );
    let blueprint=metteur_shared::dsl::compile_draft_value(&serde_json::json!({"name":"long concierge query","nodes":{"start":{"kind":"Start"},"wait":{"kind":"Delay","Ms":2000},"end":{"kind":"End"}},"flow":["start -> wait -> end"]})).unwrap();
    let delay = blueprint.nodes.iter().find(|n| n.kind == "Delay").unwrap().id;
    crate::storage::blueprint_files::save(
        &ws.db,
        &ws.version_manager,
        &blueprint,
        "long.blueprint",
        &serde_json::to_vec(&blueprint).unwrap(),
        None,
    )
    .unwrap();
    let mut execution = service
        .execute_blueprint(Request::new(crate::grpc::proto::ExecuteBlueprintRequest {
            workspace_path: ws.root.to_string_lossy().into(),
            blueprint_id: blueprint.id.to_string(),
            blueprint_json: serde_json::to_string(&blueprint).unwrap(),
        }))
        .await
        .unwrap()
        .into_inner();
    loop {
        let event = execution.next().await.unwrap().unwrap();
        if event.node_id == delay.to_string() && event.kind == "started" {
            break;
        }
    }
    let run = state.running.read().await[&ws.root].run_id;
    let mut stream = service
        .send_concierge_message(Request::new(SendConciergeMessageRequest {
            workspace_path: ws.root.to_string_lossy().into(),
            run_id: run.to_string(),
            conversation_id: run.to_string(),
            message_id: Uuid::new_v4().to_string(),
            message: "Please change the plan".into(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(stream.next().await.unwrap().unwrap().kind, "processing");
    assert_eq!(stream.next().await.unwrap().unwrap().kind, "received");
    assert_eq!(DbCheckpointSink::load(&ws.db, run).unwrap().unwrap().in_flight, Some(delay));
    while let Some(event) = execution.next().await {
        event.unwrap();
    }
    let checkpoint = DbCheckpointSink::load(&ws.db, run).unwrap().unwrap();
    assert_eq!(checkpoint.status, crate::execution::RunStatus::Completed);
    assert_eq!(checkpoint.view.invocations.iter().filter(|i| i.node_id == delay).count(), 1);
    assert_eq!(
        requests::load(&ws.db, run).unwrap().requests[0].state,
        requests::State::Failed
    );
}

#[tokio::test]
async fn reports_are_scoped_read_only_and_preserve_failed_status() {
    let (service, _, ws, run) = setup().await;
    crate::oversight::scheduler::initialize(&ws.db, run, Default::default()).unwrap();
    crate::oversight::scheduler::trigger(&ws.db, run, "checkpoint", 1).unwrap();
    let review = crate::oversight::scheduler::claim(&ws.db, run, 1).unwrap().unwrap();
    crate::oversight::scheduler::finish(&ws.db, run, review.review_id, crate::oversight::scheduler::Outcome {
        status: crate::oversight::scheduler::Status::Failed, summary: "Unavailable".into(), verdict: None, notes: vec![],
    }).unwrap();
    let request = super::super::super::proto::OversightReportsRequest { workspace_path: ws.root.to_string_lossy().into(), run_id: run.to_string() };
    let reports = service.list_oversight_reports(Request::new(request.clone())).await.unwrap().into_inner();
    let value: serde_json::Value = serde_json::from_str(&reports.reports_json).unwrap();
    assert_eq!(value["reports"][0]["status"], "failed");
    assert!(value["reports"][0]["verdict"].is_null());
    assert_eq!(value["reports"][0]["usage"], serde_json::json!([]));
    assert_eq!(value["reports"][0]["diagnostic"]["category"], "unknown_legacy");
    assert!(crate::oversight::scheduler::load(&ws.db, run).unwrap().unwrap().reviews[0].diagnostic.is_none());
    let mut other = request; other.run_id = Uuid::new_v4().to_string();
    assert_eq!(service.list_oversight_reports(Request::new(other)).await.unwrap_err().code(), tonic::Code::NotFound);
    assert!(ws.db.scan(crate::storage::persistence::cf::GRANTS).unwrap().is_empty());
}

#[tokio::test]
async fn requests_and_reviews_share_trusted_proposals_without_rewriting_records() {
    use crate::oversight::{actions, scheduler};
    let (service, state, ws, run) = setup().await;
    scheduler::initialize(&ws.db, run, Default::default()).unwrap();
    let request = requests::receive(&ws.db, run, run, Uuid::new_v4(), "Inspect first; this text is not approval", requests::Intent { category: requests::Category::Request, note: "Untrusted model summary".into() }).unwrap();
    let review = scheduler::claim(&ws.db, run, 1).unwrap().unwrap();
    let mut ctx = crate::execution::context::ExecutionContext::new(state.registry.clone(), LlmClientFactory::new(), ws.root.clone()).with_run(run, 1);
    ctx.workspace_db = Some(ws.db.clone()); ctx.config = Some(ws.config.clone());
    ctx.approvals = Some(Arc::new(crate::sandbox::approval::ApprovalBroker::new()));
    let id = Uuid::new_v4();
    let guard = actions::register(&ctx, review.review_id, id, "PauseRun", serde_json::json!({"source":"supervisor","expected":{"base":null},"summary":"Pause at boundary"})).unwrap();
    let before = serde_json::to_value(scheduler::load(&ws.db, run).unwrap()).unwrap();
    let reports = service.list_oversight_reports(Request::new(super::super::super::proto::OversightReportsRequest { workspace_path:ws.root.to_string_lossy().into(), run_id:run.to_string() })).await.unwrap().into_inner();
    let concierge = service.get_concierge_state(Request::new(ConciergeStateRequest { workspace_path:ws.root.to_string_lossy().into(), run_id:run.to_string(), conversation_id:run.to_string() })).await.unwrap().into_inner();
    let reports:serde_json::Value=serde_json::from_str(&reports.reports_json).unwrap();
    let concierge:serde_json::Value=serde_json::from_str(&concierge.state_json).unwrap();
    let proposal = &reports["reports"][0]["proposals"][0];
    assert_eq!(proposal, &concierge["reports"][0]["proposals"][0]);
    assert_eq!(proposal["original_requests"][0]["request_id"], request.request_id.to_string());
    assert_eq!(proposal["original_requests"][0]["original_text"], request.original_text);
    assert_eq!(proposal["state"], "awaiting_confirmation");
    assert!(proposal["result_version"].is_null());
    assert!(proposal["decision_source"].as_str().unwrap().is_empty());
    assert_eq!(serde_json::to_value(scheduler::load(&ws.db, run).unwrap()).unwrap(), before);
    assert!(ws.db.scan(crate::storage::persistence::cf::GRANTS).unwrap().is_empty());
    drop(guard);
}
