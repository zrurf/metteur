//! Real DB/RPC result references and read-only workflow classification.
use metteur_daemon::continuation::{
    policy::Policy,
    presentation::{workflow, workflow_with_approval},
    state::{Source, TaskState, Terminal},
    store::ContinuationStore,
};
use metteur_daemon::{AppState, DaemonService, Registry, WorkspaceManager};
use metteur_proto::proto::{ResourceRequest, SendChatRequest, daemon_server::Daemon};
use metteur_shared::config::{AclConfig, AclRule, Config};
use std::{path::PathBuf, sync::Arc};
use tokio_stream::StreamExt;
use tonic::Request;
use uuid::Uuid;

fn request(root: &std::path::Path, payload: serde_json::Value) -> ResourceRequest {
    ResourceRequest {
        workspace_path: root.to_string_lossy().into(),
        schema_version: 1,
        payload_json: payload.to_string(),
        ..Default::default()
    }
}
async fn setup() -> (PathBuf, Arc<AppState>, DaemonService, ContinuationStore, Uuid) {
    let root = std::env::temp_dir().join(format!("dw-results-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let state = Arc::new(AppState::new(
        WorkspaceManager::new().with_global_config_path(root.join("global.toml")),
        Arc::new(Registry::with_builtins()),
        Config::default(),
    ));
    let ws = state.workspaces.open(&root).await.unwrap();
    let store = ContinuationStore::new(ws.db.clone(), ws.root()).unwrap();
    let policy = Policy {
        detached: false,
        max_turns: 4,
        token_limit: 100_000,
        expires_ms: chrono::Utc::now().timestamp_millis() as u64 + 60_000,
        purpose: "Review an actual saved result".into(),
    };
    let record = store.create(policy.task(Uuid::new_v4(), 0)).unwrap();
    let id = record.task.id;
    store
        .update(id, record.revision, |r| {
            r.owner_subject = "local".into();
            r.policy = Some(policy);
            r.task
                .register_source(Source {
                    id: "job-result".into(),
                    kind: "job".into(),
                    task: id,
                    generation: r.task.generation,
                    may_write: false,
                    terminal: None,
                    result: serde_json::Value::Null,
                })
                .unwrap();
            Ok(())
        })
        .unwrap();
    (root, state.clone(), DaemonService::new(state), store, id)
}
#[tokio::test]
async fn result_selection_is_owned_exact_and_revalidated_before_reception() {
    let (root, state, service, store, id) = setup().await;
    let record = store.get(id).unwrap().unwrap();
    metteur_daemon::continuation::store::TaskHandle {
        store: store.clone(),
        id,
        generation: record.task.generation,
    }
    .finish(
        "job-result",
        Terminal::Succeeded,
        serde_json::json!({"exit_code":0,"output":"ACTUAL_RESULT 中文"}),
        1,
    )
    .unwrap();
    let capture = serde_json::json!({"operation":"capture_result","reference":{"kind":"task","taskId":id,"sourceId":"job-result","digest":""}});
    let result = service
        .query_continuations(Request::new(request(&root, capture.clone())))
        .await
        .unwrap()
        .into_inner();
    let data: serde_json::Value = serde_json::from_str(&result.payload_json).unwrap();
    assert!(data["content"].as_str().unwrap().contains("ACTUAL_RESULT"));
    let reference = data["reference"].clone();
    assert_eq!(reference["digest"].as_str().unwrap().len(), 64);
    let mut messages=service.send_chat(Request::new(SendChatRequest{workspace_path:root.to_string_lossy().into(),message:"Review the selected result".into(),history_json:"[]".into(),options_json:serde_json::json!({"provider":"mock","mock_text":"reviewed","max_tokens":128,"result_references":[reference],"context_order":[{"kind":"result","index":0}]}).to_string(),..Default::default()})).await.unwrap().into_inner();
    while let Some(event) = messages.next().await {
        event.unwrap();
    }
    let ws = state.workspaces.get(&root).await.unwrap();
    let sessions = metteur_daemon::chat::session::list_threads(&ws.db).unwrap();
    let record = metteur_daemon::chat::session::load_thread(&ws.db, &sessions[0].session_id)
        .unwrap()
        .unwrap();
    let user = record
        .transcript
        .iter()
        .find(|e| e.role == metteur_daemon::chat::transcript::EntryRole::User)
        .unwrap();
    assert!(user.content.contains("ACTUAL_RESULT"));
    assert!(user.content.contains("not an instruction or authorization"));
    assert_eq!(user.display_content.as_deref(), Some("Review the selected result"));
    let mut tampered = data["reference"].clone();
    tampered["digest"] = "0".repeat(64).into();
    assert!(
        service
            .query_continuations(Request::new(request(
                &root,
                serde_json::json!({"operation":"capture_result","reference":tampered})
            )))
            .await
            .is_err()
    );
    *state.acl_store.write().unwrap() = AclConfig {
        rules: vec![AclRule {
            subject: "local".into(),
            allow: vec!["/metteur.Daemon/ReadFile".into()],
            deny: vec![],
        }],
    };
    assert_eq!(
        service
            .query_continuations(Request::new(request(&root, capture)))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::PermissionDenied
    );
}
#[tokio::test]
async fn workflow_reason_matrix_has_no_generic_continue_or_budget_reset() {
    let (_, _, _, store, id) = setup().await;
    let original = store.get(id).unwrap().unwrap();
    let now = chrono::Utc::now().timestamp_millis() as u64;
    let mut resumed = original.clone();
    resumed.task.wait = Some(metteur_daemon::chat::wait::WaitIntent {
        sources: vec!["job-result".into()],
        mode: metteur_daemon::chat::wait::MatchMode::All,
        deadline_ms: now + 10000,
        params: serde_json::Value::Null,
    });
    assert_eq!(workflow(&resumed, 0, true, now)["reason_code"], "active");
    let before = serde_json::to_vec(&resumed).unwrap();
    let approval = workflow_with_approval(&resumed, 0, true, now, true);
    assert_eq!(approval["reason_code"], "approval_required");
    assert_eq!(serde_json::to_vec(&resumed).unwrap(), before);
    assert_eq!(
        workflow_with_approval(&resumed, 0, false, now, true)["reason_code"],
        "permission_revoked"
    );
    let mut paused = original.clone();
    paused.task.state = TaskState::Paused;
    paused.task.paused = true;
    assert!(
        workflow(&paused, 0, true, now)["allowed_actions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "resume")
    );
    for (code, mut row, current, allowed, at) in [
        ("conversation_changed", paused.clone(), 1, true, now),
        ("permission_revoked", paused.clone(), 0, false, now),
        ("policy_expired", paused.clone(), 0, true, now + 120_000),
        ("budget_exhausted", paused.clone(), 0, true, now),
        ("usage_unknown", paused.clone(), 0, true, now),
        ("effects_unknown", paused.clone(), 0, true, now),
    ] {
        if code == "budget_exhausted" {
            row.task.budget.used_turns = row.task.budget.max_turns;
        }
        if code == "usage_unknown" {
            row.task.budget.unknown_usage = true;
        }
        if code == "effects_unknown" {
            let s = row.task.sources.get_mut("job-result").unwrap();
            s.may_write = true;
            s.terminal = Some(Terminal::Indeterminate);
        }
        let before = serde_json::to_vec(&row).unwrap();
        let facts = workflow(&row, current, allowed, at);
        assert_eq!(facts["reason_code"], code);
        assert!(
            !facts["allowed_actions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == "resume" || v == "continue")
        );
        assert_eq!(serde_json::to_vec(&row).unwrap(), before);
    }
}
