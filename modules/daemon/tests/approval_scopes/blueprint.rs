use std::sync::Arc;
use std::time::Duration;

use metteur_daemon::{AppState, DaemonService, Registry, WorkspaceManager};
use metteur_proto::proto::daemon_server::Daemon;
use metteur_proto::proto::{
    ApprovalDecisionRequest, ExecuteBlueprintRequest, ListExecutionsRequest,
};
use metteur_shared::config::Config;
use tokio_stream::StreamExt;
use tonic::Request;

#[tokio::test]
async fn disconnecting_a_blueprint_stream_withdraws_its_pending_approval() {
    let root = super::common::root("blueprint-disconnect");
    let state = Arc::new(AppState::new(
        WorkspaceManager::new().with_global_config_path(root.join(".metteur/global.toml")),
        Arc::new(Registry::with_builtins()),
        Config::default(),
    ));
    state.workspaces.open(&root).await.unwrap();
    let ws = state.workspaces.get(&root).await.unwrap();
    ws.config.write().await.sandbox.enabled = true;
    let service = DaemonService::new(state);
    let blueprint = metteur_shared::dsl::compile_draft_value(&serde_json::json!({
        "name":"disconnect approval", "nodes":{
            "start":{"kind":"Start"},
            "write":{"kind":"ExecuteCommand", "command":"echo scoped > file.txt"},
            "end":{"kind":"End"}
        }, "flow":["start -> write -> end"]
    }))
    .unwrap();
    metteur_daemon::storage::blueprint_files::save(
        &ws.db,
        &ws.version_manager,
        &blueprint,
        "approval.blueprint",
        &serde_json::to_vec(&blueprint).unwrap(),
        None,
    )
    .unwrap();
    let mut stream = Daemon::execute_blueprint(
        &service,
        Request::new(ExecuteBlueprintRequest {
            workspace_path: root.to_string_lossy().into(),
            blueprint_id: blueprint.id.to_string(),
            workflow_confirmation: String::new(),
            blueprint_json: serde_json::to_string(&blueprint).unwrap(),
        }),
    )
    .await
    .unwrap()
    .into_inner();
    let id = loop {
        let event = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if event.kind == "approval_request" {
            break event.message;
        }
    };
    drop(stream);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let runs = Daemon::list_executions(
                &service,
                Request::new(ListExecutionsRequest {
                    workspace_path: root.to_string_lossy().into(),
                }),
            )
            .await
            .unwrap()
            .into_inner();
            if runs
                .executions
                .iter()
                .any(|run| matches!(run.status.as_str(), "Cancelled" | "Failed"))
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        Daemon::respond_approval(
            &service,
            Request::new(ApprovalDecisionRequest {
                workspace_path: root.to_string_lossy().into(),
                request_id: id,
                decision: "AllowWorkspace".into(),
                ..Default::default()
            })
        )
        .await
        .is_err()
    );
    assert!(!root.join("file.txt").exists());
    let store = metteur_daemon::sandbox::grant::GrantStore::new(Some(ws.db.clone()), None);
    assert_eq!(store.lookup(metteur_daemon::sandbox::command_hash("echo scoped > file.txt")), None);
}

#[tokio::test]
async fn nested_abstract_completion_keeps_the_outer_runs_grants_alive() {
    let (mut ctx, _events) = super::common::context();
    let broker = ctx.approvals.as_ref().unwrap().clone();
    broker.record_run_grant(7, true);
    let nested = metteur_shared::dsl::compile_draft_value(&serde_json::json!({
        "name":"nested", "nodes":{"start":{"kind":"Start"}, "end":{"kind":"End"}},
        "flow":["start -> end"]
    }))
    .unwrap();
    let outer = metteur_shared::dsl::compile_draft_value(&serde_json::json!({
        "name":"outer", "nodes":{
            "start":{"kind":"Start"},
            "abstract":{"kind":"Abstract", "description":"finish", "mock_text":serde_json::to_string(&nested).unwrap()}
        }, "flow":["start -> abstract"]
    })).unwrap();
    let node = outer.nodes.iter().find(|node| node.kind == "Abstract").unwrap();
    let registry = ctx.registry.clone();
    let executor = registry.node_executor("Abstract").unwrap();
    executor.execute(node, &Default::default(), &mut ctx).await.unwrap();
    assert!(!broker.is_closed());
    assert_eq!(broker.run_grant(7), Some(true));
}
