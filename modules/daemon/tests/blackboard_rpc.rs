use metteur_daemon::execution::{
    CheckpointSink, DbCheckpointSink, ExecutionCheckpoint, ExecutionEvent,
    blackboard::{self, ChangeKind, Query},
    tree::ExecTree,
};
use metteur_daemon::observability::anon::Anonymizer;
use metteur_daemon::{AppState, DaemonService, Registry, WorkspaceManager};
use metteur_proto::proto::{GetBlackboardRequest, daemon_server::Daemon};
use metteur_shared::{Blueprint, config::Config};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc};
use tonic::{Code, Request};
use uuid::Uuid;

fn checkpoint() -> ExecutionCheckpoint {
    let node = Uuid::new_v4();
    let graph = Blueprint {
        id: Uuid::new_v4(),
        name: "Board".into(),
        nodes: vec![],
        edges: vec![],
        entry_node_id: node,
    };
    let mut cp = ExecutionCheckpoint::running(Uuid::new_v4(), graph.id, 1);
    cp.view.root = Some(graph.clone());
    for at in 1..=4 {
        cp.view.begin(&graph, node, vec![], None, &HashMap::new(), at);
        let record = cp.view.invocations.last_mut().unwrap();
        record.check = Some(true);
        record.outputs = json!({"password":"never-show", "json_text":"{\"password\":\"nested-secret\"}", "text":"company-secret sk-abcdefghijklmnopqrstuvwxyz"});
        cp.view.observe(
            &ExecutionEvent::NodeFinished {
                node_id: node,
            },
            &graph.id.to_string(),
            at + 1,
        );
    }
    cp.view.invalidate(&[node], &[], ChangeKind::Rollback);
    cp
}

#[tokio::test]
async fn stored_projection_survives_reload_and_hot_truncation_keeps_evidence_lookup() {
    let cp = checkpoint();
    let anon = Anonymizer::new(&[]);
    let before = blackboard::project(
        &cp.run_id.to_string(),
        &cp.view,
        &ExecTree::default(),
        &Query::default(),
        &anon,
    )
    .await;
    let restored: ExecutionCheckpoint =
        serde_json::from_slice(&serde_json::to_vec(&cp).unwrap()).unwrap();
    let after = blackboard::project(
        &cp.run_id.to_string(),
        &restored.view,
        &restored.exec_tree,
        &Query::default(),
        &anon,
    )
    .await;
    assert_eq!(serde_json::to_value(before).unwrap(), serde_json::to_value(after).unwrap());
    let hot = blackboard::project(
        "run",
        &restored.view,
        &restored.exec_tree,
        &Query {
            last_n: 1,
            ..Default::default()
        },
        &anon,
    )
    .await;
    assert!(hot.truncated);
    assert_eq!(hot.entries.len(), 1);
    assert_eq!(hot.historical.passed_checks, 4);
    assert_eq!(hot.current.passed_checks, 0);
    assert!(!hot.entries[0].invalidates.is_empty());
    let lookup = blackboard::project(
        "run",
        &restored.view,
        &restored.exec_tree,
        &Query {
            entry_id: Some("attempt:1:check".into()),
            ..Default::default()
        },
        &anon,
    )
    .await;
    assert_eq!(lookup.entries.len(), 1);
    assert_eq!(lookup.entries[0].event, "ValidationPassed");
    assert_eq!(lookup.entries[0].validity, blackboard::Validity::Invalidated);
}

#[tokio::test]
async fn blackboard_rpc_is_workspace_scoped_redacted_bounded_and_read_only() {
    let root = std::env::temp_dir().join(format!("metteur-blackboard-{}", Uuid::new_v4()));
    let state = Arc::new(AppState::new(
        WorkspaceManager::new().with_global_config_path(root.join("global.toml")),
        Arc::new(Registry::with_builtins()),
        Config::default(),
    ));
    std::fs::create_dir_all(&root).unwrap();
    let ws = state.workspaces.open(&root).await.unwrap();
    ws.config.write().await.anonymize.extra_patterns = vec!["company-secret".into()];
    let cp = checkpoint();
    DbCheckpointSink::new(ws.db.clone(), cp.run_id).write(&cp).unwrap();
    let service = DaemonService::new(state.clone());
    let request = |query: Value| {
        Request::new(GetBlackboardRequest {
            workspace_path: root.to_string_lossy().into(),
            run_id: cp.run_id.to_string(),
            query_json: query.to_string(),
        })
    };
    let response = Daemon::get_blackboard(&service, request(json!({"last_n":10000})))
        .await
        .unwrap()
        .into_inner();
    assert!(!response.projection_json.contains("never-show"));
    assert!(!response.projection_json.contains("nested-secret"));
    assert!(!response.projection_json.contains("company-secret"));
    assert!(!response.projection_json.contains("sk-abcdefghijklmnopqrstuvwxyz"));
    let value: Value = serde_json::from_str(&response.projection_json).unwrap();
    assert_eq!(value["run_id"], cp.run_id.to_string());
    assert_eq!(value["current"]["passed_checks"], 0);
    assert_eq!(value["historical"]["passed_checks"], 4);
    assert_eq!(
        serde_json::to_value(DbCheckpointSink::load(&ws.db, cp.run_id).unwrap().unwrap()).unwrap(),
        serde_json::to_value(&cp).unwrap()
    );
    let invalid =
        Daemon::get_blackboard(&service, request(json!({"file":"/etc/passwd"}))).await.unwrap_err();
    assert_eq!(invalid.code(), Code::InvalidArgument);
    let missing =
        Daemon::get_blackboard(&service, request(json!({"entry_id":"file:///etc/passwd"})))
            .await
            .unwrap_err();
    assert_eq!(missing.code(), Code::NotFound);
    let other = root.join("other");
    std::fs::create_dir_all(&other).unwrap();
    state.workspaces.open(&other).await.unwrap();
    let foreign = Daemon::get_blackboard(
        &service,
        Request::new(GetBlackboardRequest {
            workspace_path: other.to_string_lossy().into(),
            run_id: cp.run_id.to_string(),
            query_json: "{}".into(),
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(foreign.code(), Code::NotFound);
    let audits =
        metteur_daemon::observability::audit::AuditWriter::new(ws.db.clone()).list().unwrap();
    assert!(
        audits
            .iter()
            .any(|a| a.operation == "blackboard.query"
                && a.detail["run_id"] == cp.run_id.to_string())
    );
}

#[tokio::test]
async fn oversized_hot_query_is_capped_without_losing_old_check_evidence() {
    let mut cp = checkpoint();
    let mut invocation = cp.view.invocations[0].clone();
    invocation.outputs = json!({});
    invocation.check = None;
    for sequence in 100..1200 {
        invocation.sequence = sequence;
        cp.view.invocations.push(invocation.clone());
    }
    let anon = Anonymizer::new(&[]);
    let result = blackboard::project(
        "run",
        &cp.view,
        &cp.exec_tree,
        &Query {
            last_n: usize::MAX,
            ..Default::default()
        },
        &anon,
    )
    .await;
    assert_eq!(result.entries.len(), blackboard::MAX_ENTRIES);
    assert!(result.truncated);
    assert!(!result.entries.iter().any(|e| e.id == "attempt:1:check"));
    let old = blackboard::project(
        "run",
        &cp.view,
        &cp.exec_tree,
        &Query {
            entry_id: Some("attempt:1:check".into()),
            ..Default::default()
        },
        &anon,
    )
    .await;
    assert_eq!(old.entries[0].event, "ValidationPassed");
}
