//! RPC checks for explicit rebinding; local mock provider only.
use metteur_daemon::continuation::{
    state::{TaskState, Terminal},
    store::{ContinuationStore, TaskHandle},
};
use metteur_daemon::{AppState, DaemonService, Registry, WorkspaceManager};
use metteur_proto::proto::{
    OpenWorkspaceRequest, ResourceRequest, SendChatRequest, daemon_server::Daemon,
};
use metteur_shared::config::Config;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio_stream::StreamExt;
use tonic::Request;
use uuid::Uuid;

fn req(root: &Path, payload: Value) -> ResourceRequest {
    ResourceRequest {
        workspace_path: root.to_string_lossy().into(),
        schema_version: 1,
        payload_json: payload.to_string(),
        ..Default::default()
    }
}
async fn send(
    service: &DaemonService,
    root: &Path,
    session: String,
    options: Value,
    message: &str,
) {
    let mut stream = service
        .send_chat(Request::new(SendChatRequest {
            workspace_path: root.to_string_lossy().into(),
            session_id: session,
            message: message.into(),
            options_json: options.to_string(),
            history_json: "[]".into(),
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner();
    while let Some(event) = stream.next().await {
        event.unwrap();
    }
}
async fn setup() -> (PathBuf, Arc<AppState>, DaemonService, ContinuationStore, Uuid) {
    let root = std::env::temp_dir().join(format!("dw-rebind-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let state = Arc::new(AppState::new(
        WorkspaceManager::new().with_global_config_path(root.join("global.toml")),
        Arc::new(Registry::with_builtins()),
        Config::default(),
    ));
    let ws = state.workspaces.open(&root).await.unwrap();
    let service = DaemonService::new(state.clone());
    let now = chrono::Utc::now().timestamp_millis() as u64;
    send(&service, &root, String::new(), json!({"provider":"mock","mock_text":"Initial task context","continuation_policy":{"detached":false,"purpose":"Complete the original goal with the updated constraints","max_turns":4,"token_limit":100000,"expires_ms":now+600000}}), "Original instruction").await;
    let store = ContinuationStore::new(ws.db.clone(), ws.root()).unwrap();
    let r = store.list(None).unwrap().remove(0);
    let id = r.task.id;
    let handle = TaskHandle {
        store: store.clone(),
        id,
        generation: r.task.generation,
    };
    store
        .update(id, r.revision, |r| {
            r.task.state = TaskState::Active;
            Ok(())
        })
        .unwrap();
    handle.register("already-used", "job", false).unwrap();
    handle
        .finish("already-used", Terminal::Succeeded, json!({"output":"old consumed result"}), now)
        .unwrap();
    handle.register("late-result", "job", false).unwrap();
    let r = store.get(id).unwrap().unwrap();
    store
        .update(id, r.revision, |r| {
            r.task.events.values_mut().for_each(|e| e.consumed = true);
            r.task.budget.consumed_tokens = 1200;
            // The initial mock does not report provider usage. This fixture
            // explicitly seeds a settled ledger before exercising rebinding.
            r.task.budget.unknown_usage = false;
            r.task.budget.reserved_tokens = 0;
            r.task.budget.call_ids.insert("retained-model-call".into());
            r.task
                .commit_wait(
                    metteur_daemon::chat::wait::WaitIntent {
                        sources: vec!["late-result".into()],
                        mode: metteur_daemon::chat::wait::MatchMode::All,
                        deadline_ms: now + 300000,
                        params: json!({"constraint":"original bound source"}),
                    },
                    r.task.expected_revision,
                    now,
                )
                .unwrap();
            Ok(())
        })
        .unwrap();
    send(
        &service,
        &root,
        r.task.session.to_string(),
        json!({"provider":"mock","mock_text":"Updated constraint saved"}),
        "New instruction: preserve the existing API",
    )
    .await;
    handle
        .finish("late-result", Terminal::Succeeded, json!({"output":"Late owned result"}), now + 1)
        .unwrap();
    (root, state, service, store, id)
}
async fn proposal(service: &DaemonService, root: &Path, id: Uuid) -> Value {
    let r = service
        .query_continuations(Request::new(req(root, json!({"operation":"prepare_rebind","id":id}))))
        .await
        .unwrap()
        .into_inner();
    serde_json::from_str(&r.payload_json).unwrap()
}
fn confirm(root: &Path, proposal: &Value) -> ResourceRequest {
    let mut r = req(
        root,
        json!({"id":proposal["task"],"action":"rebind","rebind_digest":proposal["digest"]}),
    );
    r.expected_revision = proposal["revision"].as_u64().unwrap();
    r.request_id = Uuid::new_v4().to_string();
    r
}

#[tokio::test]
async fn rebind_preserves_ledger_consumption_and_current_context_and_is_idempotent() {
    let (root, state, service, store, id) = setup().await;
    let before = store.get(id).unwrap().unwrap();
    let p = proposal(&service, &root, id).await;
    assert!(p["latest_user_input"].as_str().unwrap().contains("preserve the existing API"));
    assert_eq!(store.get(id).unwrap().unwrap().revision, before.revision);
    let request = confirm(&root, &p);
    service.control_continuation(Request::new(request.clone())).await.unwrap();
    let after = store.get(id).unwrap().unwrap();
    assert_eq!(
        serde_json::to_value(&after.task.budget).unwrap(),
        serde_json::to_value(&before.task.budget).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&after.task.events).unwrap(),
        serde_json::to_value(&before.task.events).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&after.task.dispatch_history).unwrap(),
        serde_json::to_value(&before.task.dispatch_history).unwrap()
    );
    assert_eq!(after.task.generation, before.task.generation);
    assert_eq!(after.task.expected_revision, p["session_revision"].as_u64().unwrap());
    assert!(
        after
            .saved_context
            .as_ref()
            .unwrap()
            .context
            .messages
            .iter()
            .any(|m| m.text_content().contains("preserve the existing API"))
    );
    service.control_continuation(Request::new(request.clone())).await.unwrap();
    assert_eq!(store.get(id).unwrap().unwrap().revision, after.revision);
    // Start the real monitor/runner using the task's recorded mock provider.
    service
        .open_workspace(Request::new(OpenWorkspaceRequest {
            path: root.to_string_lossy().into(),
        }))
        .await
        .unwrap();
    let finished = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let row = store.get(id).unwrap().unwrap();
            if row.task.state == TaskState::Completed {
                break row;
            }
            assert_ne!(row.task.state, TaskState::NeedsAttention, "{:?}", row.task.attention);
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(finished.task.budget.used_turns, before.task.budget.used_turns + 1);
    assert!(finished.task.budget.call_ids.contains("retained-model-call"));
    assert!(finished.task.budget.consumed_tokens >= 1200);
    assert_eq!(finished.task.dispatch_history.len(), before.task.dispatch_history.len() + 1);
    let dispatch = finished.task.dispatch_history.last().unwrap();
    assert_eq!(dispatch.event_ids.len(), 1);
    assert_eq!(finished.task.events[&dispatch.event_ids[0]].source_id, "late-result");
    service.control_continuation(Request::new(request)).await.unwrap();
    assert_eq!(store.get(id).unwrap().unwrap().revision, finished.revision);
    assert!(store.get(id).unwrap().unwrap().task.events.values().all(|e| e.consumed));
    assert!(state.workspaces.get(&root).await.is_some());
}

#[tokio::test]
async fn new_input_stop_permissions_and_unknown_effects_invalidate_review() {
    let (root, state, service, store, id) = setup().await;
    let p = proposal(&service, &root, id).await;
    let r = store.get(id).unwrap().unwrap();
    send(
        &service,
        &root,
        r.task.session.to_string(),
        json!({"provider":"mock","mock_text":"One more constraint"}),
        "Another new user instruction",
    )
    .await;
    assert!(service.control_continuation(Request::new(confirm(&root, &p))).await.is_err());
    assert_eq!(store.get(id).unwrap().unwrap().revision, r.revision);
    let p = proposal(&service, &root, id).await;
    let original_acl = state.acl_store.read().unwrap().clone();
    *state.acl_store.write().unwrap() = metteur_shared::config::AclConfig {
        rules: vec![metteur_shared::config::AclRule {
            subject: "local".into(),
            allow: vec!["/metteur.Daemon/ReadFile".into()],
            deny: vec![],
        }],
    };
    assert!(service.control_continuation(Request::new(confirm(&root, &p))).await.is_err());
    *state.acl_store.write().unwrap() = original_acl;
    let r = store.get(id).unwrap().unwrap();
    store
        .update(id, r.revision, |r| {
            r.task.sources.get_mut("late-result").unwrap().may_write = true;
            r.task.sources.get_mut("late-result").unwrap().terminal = Some(Terminal::Indeterminate);
            Ok(())
        })
        .unwrap();
    assert!(
        service
            .query_continuations(Request::new(req(
                &root,
                json!({"operation":"prepare_rebind","id":id})
            )))
            .await
            .is_err()
    );
    let r = store.get(id).unwrap().unwrap();
    store
        .update(id, r.revision, |r| {
            r.task.stop();
            Ok(())
        })
        .unwrap();
    assert!(service.control_continuation(Request::new(confirm(&root, &p))).await.is_err());
    assert_eq!(store.get(id).unwrap().unwrap().task.state, TaskState::Stopped);
}
