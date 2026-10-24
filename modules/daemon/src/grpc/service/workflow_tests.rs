use super::*;
use crate::continuation::{policy::Policy, store::ContinuationStore};
use metteur_shared::config::Config;
use std::sync::atomic::{AtomicBool, Ordering};
use uuid::Uuid;
use super::super::proto::ResourceRequest;

#[tokio::test]
async fn task_stop_bypasses_admission_and_cannot_cancel_another_task_on_the_same_session() {
    let root = std::env::temp_dir().join(format!("dw-task-stop-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let state = Arc::new(AppState::new(
        crate::workspace::WorkspaceManager::new().with_global_config_path(root.join("global.toml")),
        Arc::new(crate::registry::Registry::with_builtins()),
        Config::default(),
    ));
    let ws = state.workspaces.open(&root).await.unwrap();
    let store = ContinuationStore::new(ws.db.clone(), ws.root()).unwrap();
    let session = Uuid::new_v4();
    let policy = Policy {
        detached: false,
        max_turns: 4,
        token_limit: 100_000,
        expires_ms: super::continuations::now_ms() + 60_000,
        purpose: "Exact task stop".into(),
    };
    let mut ids = Vec::new();
    for _ in 0..2 {
        let r = store.create(policy.task(session, 0)).unwrap();
        ids.push(r.task.id);
        store
            .update(r.task.id, r.revision, |r| {
                r.owner_subject = "local".into();
                r.policy = Some(policy.clone());
                Ok(())
            })
            .unwrap();
    }
    let flag = Arc::new(AtomicBool::new(false));
    state.chats.write().await.insert(
        ws.root.clone(),
        ChatRun {
            continuation_task: Some(ids[1]),
            transcript: Arc::new(parking_lot::Mutex::new(
                crate::chat::transcript::Transcript::default(),
            )),
            turn_id: Uuid::new_v4(),
            approval_details: Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new())),
            interrupt_bus: None,
            cancel_requested: flag.clone(),
            approvals: None,
            persist: Arc::new(AtomicBool::new(true)),
            session_id: Some(session),
        },
    );
    let service = DaemonService::new(state);
    let _lease = ws.admit().await.unwrap();
    for (index, id) in ids.iter().enumerate() {
        let r = store.get(*id).unwrap().unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            service.control_continuation(Request::new(ResourceRequest {
                workspace_path: root.to_string_lossy().into(),
                schema_version: 1,
                expected_revision: r.revision,
                payload_json: serde_json::json!({"id":id,"action":"stop"}).to_string(),
                ..Default::default()
            })),
        )
        .await
        .expect("Stop waited for admission")
        .unwrap();
        assert_eq!(flag.load(Ordering::SeqCst), index == 1);
    }
}
