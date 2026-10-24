//! Actual RPC adapters, workspace RocksDB and the existing local mock runner.
use metteur_daemon::{
    AppState, DaemonService, Registry, WorkspaceManager, chat::session::load_thread,
};
use metteur_proto::proto::{ResourceRequest, SendChatRequest, daemon_server::Daemon};
use metteur_shared::config::Config;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio_stream::StreamExt;
use tonic::Request;
use uuid::Uuid;

async fn setup() -> (DaemonService, Arc<AppState>, PathBuf) {
    let root = std::env::temp_dir().join(format!("dw-delivery-rpc-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let state = Arc::new(AppState::new(
        WorkspaceManager::new().with_global_config_path(root.join(".metteur/global.toml")),
        Arc::new(Registry::with_builtins()),
        Config::default(),
    ));
    state.workspaces.open(&root).await.unwrap();
    (DaemonService::new(state.clone()), state, root)
}
fn command(root: &Path, id: Uuid, session: &str, expected: u64) -> SendChatRequest {
    SendChatRequest{workspace_path:root.to_string_lossy().into(),message:"manual input\r\n中文".into(),session_id:session.into(),history_json:"[]".into(),options_json:serde_json::json!({"provider":"mock","mock_text":"one answer","mock_delay_ms":150,"max_tokens":128,"delivery":{"id":id,"expected_revision":expected}}).to_string()}
}
async fn receipt(service: &DaemonService, root: &Path, id: Uuid) -> serde_json::Value {
    let result = Daemon::chat_delivery(
        service,
        Request::new(ResourceRequest {
            workspace_path: root.to_string_lossy().into(),
            schema_version: 1,
            payload_json: serde_json::json!({"operation":"read","id":id}).to_string(),
            ..Default::default()
        }),
    )
    .await
    .unwrap()
    .into_inner();
    assert!(result.error_json.is_empty(), "{}", result.error_json);
    serde_json::from_str::<serde_json::Value>(&result.payload_json).unwrap()["receipt"].clone()
}

async fn query(
    service: &DaemonService,
    root: &Path,
    payload: serde_json::Value,
) -> metteur_proto::proto::ResourceResponse {
    Daemon::chat_delivery(
        service,
        Request::new(ResourceRequest {
            workspace_path: root.to_string_lossy().into(),
            schema_version: 1,
            payload_json: payload.to_string(),
            ..Default::default()
        }),
    )
    .await
    .unwrap()
    .into_inner()
}
fn live(id: Uuid, owner: &serde_json::Value, mode: &str) -> serde_json::Value {
    serde_json::json!({"operation":"live_submit","id":id,"session":owner["session"],"turn":owner["turn"],"mode":mode,"message":"  extra\r\n中文  "})
}
#[tokio::test]
async fn live_ack_does_not_claim_consumption_when_the_running_model_finishes_without_another_call()
{
    let (service, state, root) = setup().await;
    let manual = Uuid::new_v4();
    let mut request = command(&root, manual, "", 0);
    let mut options: serde_json::Value = serde_json::from_str(&request.options_json).unwrap();
    options["mock_delay_ms"] = 500.into();
    request.options_json = options.to_string();
    let mut stream = Daemon::send_chat(&service, Request::new(request)).await.unwrap().into_inner();
    while stream.next().await.unwrap().unwrap().kind != "session" {}
    let owner = receipt(&service, &root, manual).await;
    let id = Uuid::new_v4();
    let payload = live(id, &owner, "queue");
    let first = query(&service, &root, payload.clone()).await;
    assert!(first.error_json.is_empty(), "{}", first.error_json);
    assert_eq!(receipt(&service, &root, id).await["phase"], "live_queued");
    let duplicate = query(&service, &root, payload.clone()).await;
    assert!(duplicate.error_json.is_empty(), "{}", duplicate.error_json);
    let mut conflict = payload.clone();
    conflict["message"] = "changed".into();
    assert!(query(&service, &root, conflict).await.error_json.contains("request_conflict"));
    while let Some(event) = stream.next().await {
        assert_ne!(event.unwrap().kind, "error");
    }
    assert_eq!(receipt(&service, &root, id).await["phase"], "live_unconsumed");
    let duplicate = query(&service, &root, payload).await;
    assert!(duplicate.error_json.is_empty(), "{}", duplicate.error_json);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&duplicate.payload_json).unwrap()["phase"],
        "live_unconsumed"
    );
    let ws = state.workspaces.get(&root).await.unwrap();
    let saved = load_thread(&ws.db, &Uuid::parse_str(owner["session"].as_str().unwrap()).unwrap())
        .unwrap()
        .unwrap();
    assert!(!saved.transcript.iter().any(|entry| entry.content.contains("extra")));
    let body = query(&service, &root, serde_json::json!({"operation":"live_body","id":id})).await;
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body.payload_json).unwrap()["body"],
        "  extra\r\n中文  "
    );
}
#[tokio::test]
async fn queue_and_steer_each_reach_the_root_context_and_transcript_once_with_exact_raw_input() {
    let (service, state, root) = setup().await;
    std::fs::write(root.join("fixture.txt"), "readable").unwrap();
    let manual = Uuid::new_v4();
    let mut request = command(&root, manual, "", 0);
    let mut options: serde_json::Value = serde_json::from_str(&request.options_json).unwrap();
    options["mock_delay_ms"] = 500.into();
    options["mock_steps"] = serde_json::json!([{"tool_calls":[{"name":"ReadFile","arguments":{"path":"fixture.txt"}}]},{"text":"finished"}]);
    request.options_json = options.to_string();
    let mut stream = Daemon::send_chat(&service, Request::new(request)).await.unwrap().into_inner();
    while stream.next().await.unwrap().unwrap().kind != "session" {}
    let owner = receipt(&service, &root, manual).await;
    let ids = [Uuid::new_v4(), Uuid::new_v4()];
    for (id, mode) in ids.into_iter().zip(["queue", "steer"]) {
        let payload = live(id, &owner, mode);
        for _ in 0..2 {
            let result = query(&service, &root, payload.clone()).await;
            assert!(result.error_json.is_empty(), "{}", result.error_json);
        }
    }
    while let Some(event) = stream.next().await {
        assert_ne!(event.unwrap().kind, "error");
    }
    for id in ids {
        assert_eq!(receipt(&service, &root, id).await["phase"], "live_applied");
    }
    let ws = state.workspaces.get(&root).await.unwrap();
    let saved = load_thread(&ws.db, &Uuid::parse_str(owner["session"].as_str().unwrap()).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(
        saved.transcript.iter().filter(|entry| entry.content == "  extra\r\n中文  ").count(),
        2
    );
    assert_eq!(
        saved
            .context
            .messages
            .iter()
            .filter(|message| message.text_content() == "  extra\r\n中文  ")
            .count(),
        2
    );
    Daemon::delete_chat_session(
        &service,
        Request::new(metteur_proto::proto::DeleteChatSessionRequest {
            workspace_path: root.to_string_lossy().into(),
            session_id: owner["session"].as_str().unwrap().into(),
        }),
    )
    .await
    .unwrap();
    let result =
        query(&service, &root, serde_json::json!({"operation":"live_receipts","ids":ids})).await;
    let result: serde_json::Value = serde_json::from_str(&result.payload_json).unwrap();
    assert_eq!(result["items"][0]["source_available"], false);
    let body =
        query(&service, &root, serde_json::json!({"operation":"live_body","id":ids[0]})).await;
    assert!(body.error_json.is_empty());
}
#[tokio::test]
async fn live_inputs_reject_a_retired_id_stale_turn_and_invalid_prompt_without_acceptance() {
    let (service, _state, root) = setup().await;
    let owner = serde_json::json!({"session":Uuid::new_v4(),"turn":Uuid::new_v4()});
    let id = Uuid::new_v4();
    query(&service, &root, serde_json::json!({"operation":"retire","id":id})).await;
    assert!(
        query(&service, &root, live(id, &owner, "steer"))
            .await
            .error_json
            .contains("delivery_retired")
    );
    let id = Uuid::new_v4();
    assert!(
        query(&service, &root, live(id, &owner, "queue"))
            .await
            .error_json
            .contains("target_changed")
    );
    assert!(receipt(&service, &root, id).await.is_null());
    let id = Uuid::new_v4();
    let mut payload = live(id, &owner, "steer");
    payload["prompt_snapshots"] = serde_json::json!([Uuid::new_v4()]);
    assert!(query(&service, &root, payload).await.error_json.contains("permission_denied"));
    assert!(receipt(&service, &root, id).await.is_null());
}

#[tokio::test]
async fn abandoned_live_receipts_only_reconcile_and_never_restart_a_turn() {
    use metteur_daemon::storage::delivery::{DeliveryPhase, DeliveryStore, LiveAcceptance};
    let (service, state, root) = setup().await;
    let ws = state.workspaces.get(&root).await.unwrap();
    let store = DeliveryStore::new(ws.db.clone(), &ws.root().to_string_lossy(), "local").unwrap();
    let session = Uuid::new_v4();
    let turn = Uuid::new_v4();
    for (claimed, expected) in [(false, "live_unconsumed"), (true, "live_unknown")] {
        let id = Uuid::new_v4();
        store
            .accept_live(LiveAcceptance {
                id,
                input_digest: "crash fixture",
                session,
                turn,
                mode: "queue",
                body: "unmodified crash input",
                now: 1,
            })
            .unwrap();
        if claimed {
            store.transition_live(id, DeliveryPhase::LiveClaimed, 2).unwrap();
        }
        assert_eq!(receipt(&service, &root, id).await["phase"], expected);
        assert_eq!(receipt(&service, &root, id).await["phase"], expected);
        assert_eq!(store.live_body(id).unwrap().as_deref(), Some("unmodified crash input"));
    }
    assert_eq!(admission(&service, &root).await["state"], "idle");
    assert!(metteur_daemon::chat::session::list_threads(&ws.db).unwrap().is_empty());
}

async fn admission(service: &DaemonService, root: &Path) -> serde_json::Value {
    let result = Daemon::chat_delivery(
        service,
        Request::new(ResourceRequest {
            workspace_path: root.to_string_lossy().into(),
            schema_version: 1,
            payload_json: serde_json::json!({"operation":"admission"}).to_string(),
            ..Default::default()
        }),
    )
    .await
    .unwrap()
    .into_inner();
    assert!(result.error_json.is_empty(), "{}", result.error_json);
    serde_json::from_str(&result.payload_json).unwrap()
}

#[tokio::test]
async fn targeted_stop_capability_does_not_wait_behind_workspace_admission() {
    let (service, state, root) = setup().await;
    let ws = state.workspaces.get(&root).await.unwrap();
    let _writer = ws.activity_gate.lock().await;
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        Daemon::chat_delivery(
            &service,
            Request::new(ResourceRequest {
                workspace_path: root.to_string_lossy().into(),
                schema_version: 1,
                payload_json: r#"{"operation":"capabilities"}"#.into(),
                ..Default::default()
            }),
        ),
    )
    .await
    .unwrap()
    .unwrap()
    .into_inner();
    assert!(result.error_json.is_empty());
    let data: serde_json::Value = serde_json::from_str(&result.payload_json).unwrap();
    assert_eq!(data["targeted_controls"], true);
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        Daemon::chat_delivery(
            &service,
            Request::new(ResourceRequest {
                workspace_path: root.to_string_lossy().into(),
                schema_version: 1,
                payload_json: r#"{"operation":"control_target"}"#.into(),
                ..Default::default()
            }),
        ),
    )
    .await
    .unwrap()
    .unwrap()
    .into_inner();
    assert!(result.error_json.is_empty());
    let data: serde_json::Value = serde_json::from_str(&result.payload_json).unwrap();
    assert_eq!(data["state"], "idle");
}

#[tokio::test]
async fn foreground_snapshot_uses_the_running_chat_identity_and_releases_only_when_finished() {
    let (service, _state, root) = setup().await;
    assert_eq!(admission(&service, &root).await["state"], "idle");
    let id = Uuid::new_v4();
    let mut request = command(&root, id, "", 0);
    let mut options: serde_json::Value = serde_json::from_str(&request.options_json).unwrap();
    options["mock_delay_ms"] = 1000.into();
    request.options_json = options.to_string();
    let mut stream = Daemon::send_chat(&service, Request::new(request)).await.unwrap().into_inner();
    let accepted = receipt(&service, &root, id).await;
    let owner = admission(&service, &root).await;
    assert_eq!(owner["state"], "chat");
    assert_eq!(owner["session"], accepted["session"]);
    assert_eq!(owner["turn"], accepted["turn"]);
    while let Some(event) = stream.next().await {
        event.unwrap();
    }
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while admission(&service, &root).await["state"] != "idle" {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn old_or_unqualified_chat_controls_never_target_a_later_turn_of_the_same_session() {
    let (service, state, root) = setup().await;
    let id = Uuid::new_v4();
    let mut first = Daemon::send_chat(&service, Request::new(command(&root, id, "", 0)))
        .await
        .unwrap()
        .into_inner();
    let original = receipt(&service, &root, id).await;
    let session = original["session"].as_str().unwrap().to_owned();
    let turn = original["turn"].as_str().unwrap().to_owned();
    let unqualified = metteur_proto::proto::AbortChatRequest {
        workspace_path: root.to_string_lossy().into(),
        ..Default::default()
    };
    assert_eq!(
        Daemon::abort_chat(&service, Request::new(unqualified)).await.err().unwrap().code(),
        tonic::Code::FailedPrecondition
    );
    while let Some(event) = first.next().await {
        event.unwrap();
    }
    let ws = state.workspaces.get(&root).await.unwrap();
    let revision = ws.db.chat_revision(Uuid::parse_str(&session).unwrap().as_bytes()).unwrap();
    let second_id = Uuid::new_v4();
    let mut request = command(&root, second_id, &session, revision);
    let mut options: serde_json::Value = serde_json::from_str(&request.options_json).unwrap();
    options["mock_delay_ms"] = 1000.into();
    request.options_json = options.to_string();
    let mut second = Daemon::send_chat(&service, Request::new(request)).await.unwrap().into_inner();
    let current = receipt(&service, &root, second_id).await;
    assert_ne!(current["turn"], original["turn"]);
    assert_eq!(
        Daemon::abort_chat(
            &service,
            Request::new(metteur_proto::proto::AbortChatRequest {
                workspace_path: root.to_string_lossy().into(),
                session_id: session.clone(),
                turn_id: turn.clone(),
                ..Default::default()
            })
        )
        .await
        .err()
        .unwrap()
        .code(),
        tonic::Code::FailedPrecondition
    );
    for priority in ["Normal", "Urgent", "Emergency"] {
        assert_eq!(
            Daemon::send_interrupt(
                &service,
                Request::new(metteur_proto::proto::InterruptRequest {
                    workspace_path: root.to_string_lossy().into(),
                    priority: priority.into(),
                    message: "must not reach B".into(),
                    target_kind: "chat".into(),
                    session_id: session.clone(),
                    turn_id: turn.clone(),
                    run_id: String::new(),
                    ..Default::default()
                })
            )
            .await
            .err()
            .unwrap()
            .code(),
            tonic::Code::FailedPrecondition
        );
    }
    let unqualified = metteur_proto::proto::InterruptRequest {
        workspace_path: root.to_string_lossy().into(),
        priority: "Urgent".into(),
        message: "ambiguous".into(),
        ..Default::default()
    };
    assert_eq!(
        Daemon::send_interrupt(&service, Request::new(unqualified)).await.err().unwrap().code(),
        tonic::Code::FailedPrecondition
    );
    while let Some(event) = second.next().await {
        assert_ne!(event.unwrap().kind, "error", "The stale Stop must not cancel B");
    }
    assert_eq!(receipt(&service, &root, second_id).await["outcome"], "done");
}

#[tokio::test]
async fn blueprint_pause_keeps_foreground_admission_and_legacy_send_is_rejected() {
    let (service, _state, root) = setup().await;
    let graph=metteur_shared::dsl::compile_draft_value_with_catalog(&serde_json::json!({"name":"foreground fixture","nodes":{"s":{"kind":"Start"},"d":{"kind":"Delay","Ms":1500},"e":{"kind":"End"}},"flow":["s -> d -> e"]}),&Registry::with_builtins().authoring_catalog()).unwrap();
    let file =
        String::from_utf8(metteur_daemon::storage::blueprint_files::encode_native(&graph).unwrap())
            .unwrap();
    Daemon::write_file(
        &service,
        Request::new(metteur_proto::proto::WriteFileRequest {
            workspace_path: root.to_string_lossy().into(),
            path: "foreground.blueprint".into(),
            content: file.clone(),
            ..Default::default()
        }),
    )
    .await
    .unwrap();
    let mut stream = Daemon::execute_blueprint(
        &service,
        Request::new(metteur_proto::proto::ExecuteBlueprintRequest {
            workspace_path: root.to_string_lossy().into(),
            blueprint_id: graph.id.to_string(),
            workflow_confirmation: String::new(),
            blueprint_json: file,
        }),
    )
    .await
    .unwrap()
    .into_inner();
    let owner = admission(&service, &root).await;
    assert_eq!(owner["state"], "blueprint");
    let run = owner["run"].as_str().unwrap().to_owned();
    let wrong_channel = metteur_proto::proto::InterruptRequest {
        workspace_path: root.to_string_lossy().into(),
        priority: "Urgent".into(),
        message: "Main chat must not enter supervision".into(),
        target_kind: "chat".into(),
        session_id: Uuid::new_v4().to_string(),
        turn_id: Uuid::new_v4().to_string(),
        run_id: String::new(),
        ..Default::default()
    };
    assert!(Daemon::send_interrupt(&service, Request::new(wrong_channel)).await.is_err());
    assert_eq!(
        Daemon::pause_execution(
            &service,
            Request::new(metteur_proto::proto::PauseRequest {
                workspace_path: root.to_string_lossy().into(),
                run_id: String::new(),
                receipt_json: serde_json::json!({"id":uuid::Uuid::new_v4(),"revision":0}).to_string(),
            })
        )
        .await
        .err()
        .unwrap()
        .code(),
        tonic::Code::FailedPrecondition
    );
    Daemon::pause_execution(
        &service,
        Request::new(metteur_proto::proto::PauseRequest {
            workspace_path: root.to_string_lossy().into(),
            run_id: run.clone(),
            receipt_json: serde_json::json!({"id":uuid::Uuid::new_v4(),"revision":0}).to_string(),
        }),
    )
    .await
    .unwrap();
    assert_eq!(admission(&service, &root).await["reason"], "Blueprint is paused");
    let mut legacy = command(&root, Uuid::new_v4(), "", 0);
    legacy.options_json = r#"{"provider":"mock"}"#.into();
    assert_eq!(
        Daemon::send_chat(&service, Request::new(legacy)).await.err().unwrap().code(),
        tonic::Code::FailedPrecondition
    );
    Daemon::cancel_execution(
        &service,
        Request::new(metteur_proto::proto::CancelRequest {
            workspace_path: root.to_string_lossy().into(),
            run_id: run.clone(),
            ..Default::default()
        }),
    )
    .await
    .unwrap();
    assert_eq!(admission(&service, &root).await["state"], "blueprint");
    assert!(
        Daemon::send_interrupt(
            &service,
            Request::new(metteur_proto::proto::InterruptRequest {
                workspace_path: root.to_string_lossy().into(),
                priority: "Urgent".into(),
                message: "late input after Stop".into(),
                target_kind: "blueprint".into(),
                run_id: run,
                session_id: String::new(),
                turn_id: String::new(),
                ..Default::default()
            })
        )
        .await
        .is_err()
    );
    let mut aborted = false;
    while let Some(event) = stream.next().await {
        if let Err(error) = event {
            assert_eq!(error.code(), tonic::Code::Aborted);
            aborted = true;
        }
    }
    assert!(aborted, "Cancel must have its real interrupted outcome");
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while admission(&service, &root).await["state"] != "idle" {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn accepted_input_is_durable_before_the_first_stream_event_and_duplicate_never_runs_again() {
    let (service, state, root) = setup().await;
    let id = Uuid::new_v4();
    let request = command(&root, id, "", 0);
    let mut stream =
        Daemon::send_chat(&service, Request::new(request.clone())).await.unwrap().into_inner();
    let accepted = receipt(&service, &root, id).await;
    let session = Uuid::parse_str(accepted["session"].as_str().unwrap()).unwrap();
    let ws = state.workspaces.get(&root).await.unwrap();
    let persisted = load_thread(&ws.db, &session).unwrap().unwrap();
    assert_eq!(persisted.context.messages.last().unwrap().text_content(), "manual input\r\n中文");
    let first = stream.next().await.unwrap().unwrap();
    assert_eq!(first.kind, "delivery");
    let mut replay =
        Daemon::send_chat(&service, Request::new(request.clone())).await.unwrap().into_inner();
    let mut replay_kinds = vec![];
    while let Some(event) = replay.next().await {
        replay_kinds.push(event.unwrap().kind);
    }
    assert_eq!(replay_kinds, vec!["delivery", "delivery_replay"]);
    while let Some(event) = stream.next().await {
        assert_ne!(event.unwrap().kind, "error");
    }
    let finished = receipt(&service, &root, id).await;
    assert_eq!(finished["phase"], "finished");
    assert_eq!(finished["turn"], accepted["turn"]);
    assert_eq!(load_thread(&ws.db, &session).unwrap().unwrap().turns, 1);
    let mut late = Daemon::send_chat(&service, Request::new(request)).await.unwrap().into_inner();
    assert_eq!(late.next().await.unwrap().unwrap().kind, "delivery");
    assert_eq!(load_thread(&ws.db, &session).unwrap().unwrap().turns, 1);
}

#[tokio::test]
async fn changed_input_and_stale_revision_are_rejected_without_advancing_the_session() {
    let (service, state, root) = setup().await;
    let id = Uuid::new_v4();
    let request = command(&root, id, "", 0);
    let mut stream =
        Daemon::send_chat(&service, Request::new(request.clone())).await.unwrap().into_inner();
    while let Some(event) = stream.next().await {
        event.unwrap();
    }
    let accepted = receipt(&service, &root, id).await;
    let session = Uuid::parse_str(accepted["session"].as_str().unwrap()).unwrap();
    let ws = state.workspaces.get(&root).await.unwrap();
    let revision = ws.db.chat_revision(session.as_bytes()).unwrap();
    let mut changed = request;
    changed.message = "different input".into();
    assert!(
        Daemon::send_chat(&service, Request::new(changed))
            .await
            .err()
            .unwrap()
            .message()
            .contains("request_conflict")
    );
    let rejected = Uuid::new_v4();
    assert!(
        Daemon::send_chat(
            &service,
            Request::new(command(&root, rejected, &session.to_string(), 0))
        )
        .await
        .is_err()
    );
    assert!(receipt(&service, &root, rejected).await.is_null());
    assert_eq!(ws.db.chat_revision(session.as_bytes()).unwrap(), revision);
}

#[tokio::test]
async fn accepted_input_and_continuation_expected_revision_commit_together() {
    let (service, _state, root) = setup().await;
    let id = Uuid::new_v4();
    let mut request = command(&root, id, "", 0);
    let mut options: serde_json::Value = serde_json::from_str(&request.options_json).unwrap();
    options["continuation_policy"] = serde_json::json!({"detached":false,"max_turns":2,"token_limit":100000,"expires_ms":chrono::Utc::now().timestamp_millis()+60000,"purpose":"bounded fixture"});
    request.options_json = options.to_string();
    let mut stream = Daemon::send_chat(&service, Request::new(request)).await.unwrap().into_inner();
    while let Some(event) = stream.next().await {
        let event = event.unwrap();
        assert_ne!(event.kind, "error", "{}", event.content);
    }
    assert_eq!(receipt(&service, &root, id).await["phase"], "finished");
}

#[tokio::test]
async fn unreceived_identity_retirement_fences_a_late_send_without_creating_a_session() {
    let (service, state, root) = setup().await;
    let id = Uuid::new_v4();
    let result = Daemon::chat_delivery(
        &service,
        Request::new(ResourceRequest {
            workspace_path: root.to_string_lossy().into(),
            schema_version: 1,
            payload_json: serde_json::json!({"operation":"retire","id":id}).to_string(),
            ..Default::default()
        }),
    )
    .await
    .unwrap()
    .into_inner();
    assert!(result.error_json.is_empty());
    assert!(
        Daemon::send_chat(&service, Request::new(command(&root, id, "", 0)))
            .await
            .err()
            .unwrap()
            .message()
            .contains("delivery_retired")
    );
    let ws = state.workspaces.get(&root).await.unwrap();
    assert!(metteur_daemon::chat::session::list_threads(&ws.db).unwrap().is_empty());
}

#[tokio::test]
async fn new_chat_identity_is_bound_before_acceptance_and_invalid_model_is_not_accepted() {
    let (service, _state, root) = setup().await;
    let id = Uuid::new_v4();
    let session = Uuid::new_v4();
    let mut request = command(&root, id, "", 0);
    let mut options: serde_json::Value = serde_json::from_str(&request.options_json).unwrap();
    options["delivery"]["new_session"] = serde_json::json!(session);
    request.options_json = options.to_string();
    let mut stream = Daemon::send_chat(&service, Request::new(request)).await.unwrap().into_inner();
    while let Some(event) = stream.next().await {
        let event = event.unwrap();
        assert_ne!(event.kind, "error", "{}", event.content);
    }
    assert_eq!(receipt(&service, &root, id).await["session"], session.to_string());
    let rejected = Uuid::new_v4();
    let mut request = command(&root, rejected, "", 0);
    let mut options: serde_json::Value = serde_json::from_str(&request.options_json).unwrap();
    options["provider"] = serde_json::json!("unconfigured-provider");
    options["model"] = serde_json::json!("deleted-model");
    request.options_json = options.to_string();
    assert!(
        Daemon::send_chat(&service, Request::new(request))
            .await
            .err()
            .unwrap()
            .message()
            .contains("Selected model is no longer configured")
    );
    assert!(receipt(&service, &root, rejected).await.is_null());
}

#[tokio::test]
async fn host_backed_pending_inputs_are_reconciled_before_another_browser_can_resubmit() {
    let (service, state, root) = setup().await;
    let ws = state.workspaces.get(&root).await.unwrap();
    let recovery = metteur_daemon::storage::recovery::RecoveryStore::new(
        ws.db.clone(),
        &format!("project:{}", ws.root().to_string_lossy()),
        "local",
    )
    .unwrap();
    let unreceived = Uuid::new_v4();
    let received = Uuid::new_v4();
    let mut stream = Daemon::send_chat(&service, Request::new(command(&root, received, "", 0)))
        .await
        .unwrap()
        .into_inner();
    while let Some(event) = stream.next().await {
        assert_ne!(event.unwrap().kind, "error");
    }
    let original_receipt = receipt(&service, &root, received).await;
    let mut copies = vec![];
    for (kind, submission) in
        [("chat", unreceived), ("new_chat", received), ("supervision", Uuid::new_v4())]
    {
        let draft = metteur_daemon::storage::recovery::DraftInput {
            creation_epoch: None,
            id: Uuid::new_v4(),
            window_id: Uuid::new_v4(),
            kind: kind.into(),
            entity: Uuid::new_v4().to_string(),
            edit_sequence: 1,
            raw: "unmodified raw\r\n中文".into(),
            baseline_identity: None,
            selections: vec![],
            pending_submission: Some(submission),
            selected_model: None,
            view: None,
        };
        copies.push(draft.id);
        recovery.backup(draft, 0, Uuid::new_v4(), 1).unwrap();
    }
    let result = Daemon::chat_delivery(
        &service,
        Request::new(ResourceRequest {
            workspace_path: root.to_string_lossy().into(),
            schema_version: 1,
            payload_json: serde_json::json!({"operation":"reconcile_pending"}).to_string(),
            ..Default::default()
        }),
    )
    .await
    .unwrap()
    .into_inner();
    assert!(result.error_json.is_empty(), "{}", result.error_json);
    let result: serde_json::Value = serde_json::from_str(&result.payload_json).unwrap();
    assert_eq!(result["items"].as_array().unwrap().len(), 2);
    assert_eq!(receipt(&service, &root, unreceived).await["phase"], "retired");
    assert_eq!(receipt(&service, &root, received).await["turn"], original_receipt["turn"]);
    assert_eq!(receipt(&service, &root, received).await["phase"], "finished");
    assert!(
        Daemon::send_chat(&service, Request::new(command(&root, unreceived, "", 0))).await.is_err()
    );
    for id in copies {
        assert_eq!(recovery.get(id).unwrap().unwrap().input.raw, "unmodified raw\r\n中文");
    }
}

#[tokio::test]
async fn receipt_reconciliation_reports_a_deleted_source_without_recreating_it() {
    let (service, state, root) = setup().await;
    let id = Uuid::new_v4();
    let mut stream = Daemon::send_chat(&service, Request::new(command(&root, id, "", 0)))
        .await
        .unwrap()
        .into_inner();
    while let Some(event) = stream.next().await {
        assert_ne!(event.unwrap().kind, "error");
    }
    let query = || ResourceRequest {
        workspace_path: root.to_string_lossy().into(),
        schema_version: 1,
        payload_json: serde_json::json!({"operation":"read","id":id}).to_string(),
        ..Default::default()
    };
    let before = Daemon::chat_delivery(&service, Request::new(query())).await.unwrap().into_inner();
    let before: serde_json::Value = serde_json::from_str(&before.payload_json).unwrap();
    assert_eq!(before["source_available"], true);
    let session = before["receipt"]["session"].as_str().unwrap();
    Daemon::delete_chat_session(
        &service,
        Request::new(metteur_proto::proto::DeleteChatSessionRequest {
            workspace_path: root.to_string_lossy().into(),
            session_id: session.into(),
        }),
    )
    .await
    .unwrap();
    let after = Daemon::chat_delivery(&service, Request::new(query())).await.unwrap().into_inner();
    let after: serde_json::Value = serde_json::from_str(&after.payload_json).unwrap();
    assert_eq!(after["source_available"], false);
    assert_eq!(after["receipt"]["turn"], before["receipt"]["turn"]);
    let ws = state.workspaces.get(&root).await.unwrap();
    assert!(load_thread(&ws.db, &Uuid::parse_str(session).unwrap()).unwrap().is_none());
}
