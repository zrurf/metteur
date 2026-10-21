//! Final local gaps: real session replacement and restart with an unknown job.
use metteur_daemon::{
    AppState, DaemonService, Registry, WorkspaceManager,
    chat::{
        session::{ChatSessionRecord, load_thread, save_thread},
        wait::{MatchMode, WaitIntent},
    },
    continuation::{
        policy::{Policy, binding_digest},
        state::{Source, TaskState, Terminal},
        store::{ContinuationStore, TaskHandle},
    },
    storage::persistence::Db,
};
use metteur_proto::proto::{OpenWorkspaceRequest, SendChatRequest, daemon_server::Daemon};
use metteur_shared::{
    config::Config,
    llm::{ContextManager, Message, Role},
};
use std::sync::Arc;
use tokio_stream::StreamExt;
use tonic::Request;
use uuid::Uuid;

fn seed_wait(
    db: Db,
    root: &std::path::Path,
    registry: &Registry,
    config: &Config,
    kind: &str,
    may_write: bool,
) -> (ContinuationStore, Uuid, Uuid) {
    let mut context = ContextManager::default();
    context.messages.push(Message::text(Role::User, "OLD_SYNTHETIC_CONTEXT"));
    let saved = ChatSessionRecord::new(1, context);
    save_thread(&db, &saved).unwrap();
    let policy = Policy {
        detached: true,
        max_turns: 4,
        token_limit: 100000,
        expires_ms: chrono::Utc::now().timestamp_millis() as u64 + 60000,
        purpose: "Synthetic boundary verification".into(),
    };
    let mut task =
        policy.task(saved.session_id, db.chat_revision(saved.session_id.as_bytes()).unwrap());
    let id = task.id;
    task.register_source(Source {
        id: "owned-source".into(),
        kind: kind.into(),
        task: id,
        generation: task.generation,
        may_write,
        terminal: None,
        result: serde_json::json!({"old_pid":1}),
    })
    .unwrap();
    let store = ContinuationStore::new(db, root).unwrap();
    let r = store.create(task).unwrap();
    let r = store.update(id, r.revision, |r| {
        r.owner_subject = "local".into();
        r.policy = Some(policy);
        r.model_options = serde_json::json!({"provider":"mock","mock_text":"OLD_CONTEXT_MUST_NOT_RESUME","max_tokens":128});
        r.binding_digest = Some(binding_digest(config, None, registry));
        Ok(())
    }).unwrap();
    store
        .save_wait(
            id,
            r.revision,
            saved.clone(),
            WaitIntent {
                sources: vec!["owned-source".into()],
                mode: MatchMode::Any,
                deadline_ms: chrono::Utc::now().timestamp_millis() as u64 + 60000,
                params: serde_json::json!({}),
            },
            2,
        )
        .unwrap();
    (store, id, saved.session_id)
}

#[tokio::test]
async fn newer_user_message_fences_and_retires_the_old_wait_context() {
    let root = std::env::temp_dir().join(format!("nx-final-message-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let registry = Arc::new(Registry::with_builtins());
    let state = Arc::new(AppState::new(
        WorkspaceManager::new().with_global_config_path(root.join("isolated.toml")),
        registry.clone(),
        Config::default(),
    ));
    let service = DaemonService::new(state.clone());
    Daemon::open_workspace(
        &service,
        Request::new(OpenWorkspaceRequest {
            path: root.to_string_lossy().into_owned(),
        }),
    )
    .await
    .unwrap();
    let ws = state.workspaces.get(&root).await.unwrap();
    let effective = ws.config.read().await.clone();
    let (store, id, session) =
        seed_wait(ws.db.clone(), &root, &registry, &effective, "timer", false);
    let mut stream = Daemon::send_chat(&service, Request::new(SendChatRequest { workspace_path: root.to_string_lossy().into_owned(), session_id: session.to_string(), message: "NEW_USER_INSTRUCTION".into(), options_json: serde_json::json!({"provider":"mock","mock_text":"LATEST_USER_RESULT","max_tokens":128}).to_string(), ..Default::default() })).await.unwrap().into_inner();
    while let Some(event) = stream.next().await {
        assert_ne!(event.unwrap().kind, "error");
    }
    let before = load_thread(&ws.db, &session).unwrap().unwrap();
    assert!(
        before.context.messages.iter().any(|m| m.text_content().contains("LATEST_USER_RESULT"))
    );
    TaskHandle {
        store: store.clone(),
        id,
        generation: 1,
    }
    .finish("owned-source", Terminal::Succeeded, serde_json::json!({"source":"old"}), 3)
    .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(4), async {
        loop {
            if store.get(id).unwrap().unwrap().task.state == TaskState::NeedsAttention {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("Old context must be retired, not repeatedly admitted forever");
    assert!(
        store
            .get(id)
            .unwrap()
            .unwrap()
            .task
            .attention
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains("session")
    );
    let after = load_thread(&ws.db, &session).unwrap().unwrap();
    assert_eq!(serde_json::to_value(&before).unwrap(), serde_json::to_value(&after).unwrap());
    assert!(store.get(id).unwrap().unwrap().task.dispatch_history.is_empty());
}

#[test]
fn restarted_job_is_indeterminate_without_pid_reuse_or_replay() {
    let root = std::env::temp_dir().join(format!("nx-final-job-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let registry = Registry::with_builtins();
    let db = Db::open(&root.join("db")).unwrap();
    let (store, id, session) =
        seed_wait(db.clone(), &root, &registry, &Config::default(), "job", true);
    let before = load_thread(&db, &session).unwrap().unwrap();
    drop(store);
    drop(db);
    let db = Db::open(&root.join("db")).unwrap();
    let store = ContinuationStore::new(db.clone(), &root).unwrap();
    store.recover().unwrap();
    let first = store.get(id).unwrap().unwrap();
    assert_eq!(first.task.sources["owned-source"].terminal, Some(Terminal::Indeterminate));
    assert_eq!(first.task.sources["owned-source"].result["automatic_replay"], false);
    assert!(first.task.dispatch.is_none());
    assert!(first.task.dispatch_history.is_empty());
    let mut task = first.task.clone();
    assert!(task.claim(db.chat_revision(session.as_bytes()).unwrap(), 5, 0).is_err());
    store.recover().unwrap();
    let second = store.get(id).unwrap().unwrap();
    assert_eq!(first.task.events.len(), second.task.events.len());
    assert_eq!(
        serde_json::to_value(before).unwrap(),
        serde_json::to_value(load_thread(&db, &session).unwrap().unwrap()).unwrap()
    );
}
