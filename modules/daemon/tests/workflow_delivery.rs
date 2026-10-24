//! Workflow acceptance uses real commands, durable checkpoints and user-only CAS.
//! Mock text is only the implementation-stage provider; it never supplies a check result.
use metteur_daemon::execution::checkpoint::RunStatus;
use metteur_daemon::execution::{
    CheckpointSink, DbCheckpointSink, ExecutionCheckpoint, Interpreter,
};
use metteur_daemon::llm::{LlmClientFactory, MockClient};
use metteur_daemon::workflow::{
    evidence::capture,
    projection::project,
    reviews::{Decision, ReviewInput, ReviewStore},
};
use metteur_daemon::{AppState, DaemonService, Registry, WorkspaceManager};
use metteur_proto::proto::{ResourceRequest, daemon_server::Daemon};
use metteur_shared::{
    config::{AclConfig, AclRule, Config},
    workflow::*,
};
use serde_json::json;
use std::{path::PathBuf, sync::Arc};
use tonic::Request;
use uuid::Uuid;

fn input() -> TemplateInput {
    TemplateInput {
        instance_id: Uuid::new_v4(),
        template: TemplateKind::FixBug,
        name: "Durable workflow".into(),
        goal: "Verify a real process and review its file evidence".into(),
        constraints: "Keep scope bounded".into(),
        scope_paths: vec!["source.txt".into()],
        evidence_paths: vec!["source.txt".into()],
        model: "fixture".into(),
        max_iterations: 2,
        baseline: CommandCheck {
            id: "baseline".into(),
            description: "Expected process failure".into(),
            command: "exit 1".into(),
            expected_exit_code: 1,
            required: true,
        },
        checks: vec![CommandCheck {
            id: "final".into(),
            description: "Final process succeeds".into(),
            command: "exit 0".into(),
            expected_exit_code: 0,
            required: true,
        }],
        manual_checks: vec![ManualCheck {
            id: "scope".into(),
            description: "Review scope".into(),
            required: true,
        }],
    }
}
async fn setup() -> (PathBuf, Arc<AppState>) {
    let root = std::env::temp_dir().join(format!("workflow-delivery-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("source.txt"), "original bytes").unwrap();
    let state = Arc::new(AppState::new(
        WorkspaceManager::new().with_global_config_path(root.join("global.toml")),
        Arc::new(Registry::with_builtins()),
        Config::default(),
    ));
    state.workspaces.open(&root).await.unwrap();
    (root, state)
}
async fn execute(
    root: &std::path::Path,
    state: &AppState,
    input: &TemplateInput,
) -> ExecutionCheckpoint {
    let ws = state.workspaces.get(root).await.unwrap();
    let registry = Arc::new(Registry::with_builtins());
    let graph = compile_template(input, &registry.authoring_catalog()).unwrap();
    metteur_daemon::storage::blueprint_files::save(
        &ws.db,
        &ws.version_manager,
        &graph,
        "workflow.blueprint",
        &serde_json::to_vec(&graph).unwrap(),
        None,
    )
    .unwrap();
    let run = Uuid::new_v4();
    let mut runner = Interpreter::new(
        registry,
        LlmClientFactory::with_override(Arc::new(MockClient::text(
            "Implementation-stage fixture; no acceptance claim.",
        ))),
        root.to_path_buf(),
    )
    .with_workspace_db(ws.db.clone())
    .with_version_manager(ws.version_manager.clone())
    .with_config(ws.config.clone())
    .with_jobs(ws.jobs.clone())
    .with_checkpoint_sink(Arc::new(DbCheckpointSink::new(ws.db.clone(), run)));
    let shared = Arc::new(parking_lot::RwLock::new(graph));
    tokio::time::timeout(std::time::Duration::from_secs(20), runner.run(&shared, None))
        .await
        .unwrap()
        .unwrap();
    DbCheckpointSink::load(&ws.db, run).unwrap().unwrap()
}
fn decision(review: &metteur_daemon::workflow::projection::WorkflowReview) -> ReviewInput {
    ReviewInput {
        run_id: review.run_id,
        evidence_digest: review.digest.clone(),
        criterion_id: "final".into(),
        decision: Decision::Accepted,
        note: "Inspected actual process and file evidence".into(),
    }
}
fn request(root: &std::path::Path, payload: serde_json::Value) -> ResourceRequest {
    ResourceRequest {
        workspace_path: root.to_string_lossy().into(),
        schema_version: 1,
        payload_json: payload.to_string(),
        ..Default::default()
    }
}

#[tokio::test]
async fn real_process_results_are_version_bound_and_file_changes_invalidate_only_current_acceptance()
 {
    let (root, state) = setup().await;
    let cp = execute(&root, &state, &input()).await;
    assert_eq!(cp.status, RunStatus::Completed);
    let review = project(&root, &cp).unwrap();
    assert_eq!(
        review.criteria.iter().map(|r| r.status.as_str()).collect::<Vec<_>>(),
        vec!["passed", "passed", "needs_review"]
    );
    assert!(cp.blueprint_version.is_some());
    assert_eq!(cp.view.workflow_evidence.len(), 2);
    assert!(review.criteria[1].output.as_ref().unwrap().contains("exited"));
    let before = serde_json::to_vec(&cp).unwrap();
    std::fs::write(root.join("source.txt"), "changed bytes").unwrap();
    let changed = project(&root, &cp).unwrap();
    assert_ne!(review.digest, changed.digest);
    assert_eq!(changed.criteria[0].status, "passed");
    assert_eq!(changed.criteria[1].status, "stale");
    std::fs::remove_file(root.join("source.txt")).unwrap();
    assert_eq!(project(&root, &cp).unwrap().criteria[2].status, "missing");
    assert_eq!(serde_json::to_vec(&cp).unwrap(), before, "Query must never rewrite history");
    std::fs::write(root.join("source.txt"), "original bytes").unwrap();
    let mut broken = cp.clone();
    broken.blueprint_version = None;
    assert_eq!(project(&root, &broken).unwrap().criteria[1].status, "stale");
    let contract = read_contract(cp.view.root.as_ref().unwrap()).unwrap().unwrap();
    let CheckBinding::Command {
        command_node,
        validator_node,
        ..
    } = contract.criteria[1].check
    else {
        panic!()
    };
    let mut broken = cp.clone();
    broken.view.invocations.iter_mut().find(|r| r.node_id == command_node).unwrap().outputs =
        json!({});
    assert_eq!(
        project(&root, &broken).unwrap().criteria[1].status,
        "missing",
        "A check boolean alone is not proof"
    );
    let mut broken = cp.clone();
    broken.view.invocations.iter_mut().find(|r| r.node_id == validator_node).unwrap().current =
        false;
    assert_eq!(project(&root, &broken).unwrap().criteria[1].status, "missing");
    let mut broken = cp.clone();
    broken.view.workflow_evidence.clear();
    assert_eq!(project(&root, &broken).unwrap().criteria[1].status, "stale");
}

#[tokio::test]
async fn wrong_baseline_blocks_implementation_and_nonzero_final_remains_failed() {
    let (root, state) = setup().await;
    let mut p = input();
    p.baseline.command = "exit 2".into();
    let cp = execute(&root, &state, &p).await;
    let result = project(&root, &cp).unwrap();
    assert_eq!(result.criteria[0].status, "failed");
    assert_eq!(result.criteria[1].status, "missing");
    assert!(
        cp.view.invocations.iter().all(|i| cp
            .view
            .root
            .as_ref()
            .unwrap()
            .node(i.node_id)
            .unwrap()
            .kind
            != "CallLLM")
    );
    let (root, state) = setup().await;
    let mut p = input();
    p.checks[0].command = "exit 1".into();
    let cp = execute(&root, &state, &p).await;
    assert_eq!(project(&root, &cp).unwrap().criteria[1].status, "failed");
}

#[tokio::test]
async fn review_receipts_survive_reopen_with_conflicts_stale_files_and_subject_isolation() {
    let (root, state) = setup().await;
    let cp = execute(&root, &state, &input()).await;
    let ws = state.workspaces.get(&root).await.unwrap();
    let store = ReviewStore::new(ws.db.clone(), &root, "local").unwrap();
    let review = project(&root, &cp).unwrap();
    let input = decision(&review);
    let id = Uuid::new_v4();
    assert_eq!(store.record(input.clone(), 0, id, &review).unwrap().revision, 1);
    assert_eq!(store.record(input.clone(), 0, id, &review).unwrap().revision, 1);
    let mut changed = input.clone();
    changed.note = "different".into();
    assert_eq!(store.record(changed, 0, id, &review).unwrap_err().code, "request_conflict");
    assert_eq!(
        store.record(input.clone(), 0, Uuid::new_v4(), &review).unwrap_err().code,
        "revision_conflict"
    );
    assert_eq!(
        ReviewStore::new(ws.db.clone(), &root, "other-user")
            .unwrap()
            .read(review.clone())
            .unwrap()
            .revision,
        0
    );
    let mut too_long = input.clone();
    too_long.note = "x".repeat(2049);
    assert_eq!(
        store.record(too_long, 1, Uuid::new_v4(), &review).unwrap_err().code,
        "invalid_request"
    );
    for revision in 1..64 {
        let mut next = input.clone();
        next.note = format!("Review {revision}");
        store.record(next, revision, Uuid::new_v4(), &review).unwrap();
    }
    assert_eq!(
        store.record(input.clone(), 64, Uuid::new_v4(), &review).unwrap_err().code,
        "resource_limit"
    );
    assert_eq!(
        store.receipt(id).unwrap().unwrap().revision,
        1,
        "Historical receipts are immutable"
    );
    std::fs::write(root.join("source.txt"), "later edit").unwrap();
    let stale = project(&root, &cp).unwrap();
    assert_eq!(
        store.read(stale.clone()).unwrap().criteria[1].decision.as_ref().unwrap()["current"],
        false
    );
    assert_eq!(store.record(input.clone(), 64, id, &stale).unwrap_err().code, "request_conflict");
    assert_eq!(
        store.record(input, 0, id, &stale).unwrap().revision,
        1,
        "Exact replay survives evidence changes"
    );
    drop(store);
    drop(ws);
    state.workspaces.close(&root).await.unwrap();
    let reopened = state.workspaces.open(&root).await.unwrap();
    let store = ReviewStore::new(reopened.db.clone(), &root, "local").unwrap();
    assert_eq!(
        store
            .read(
                project(&root, &DbCheckpointSink::load(&reopened.db, cp.run_id).unwrap().unwrap())
                    .unwrap()
            )
            .unwrap()
            .revision,
        64
    );
    assert_eq!(store.receipt(id).unwrap().unwrap().revision, 1);
}

#[tokio::test]
async fn rpc_enforces_workspace_acl_user_fields_redaction_and_query_has_no_write_effect() {
    let (root, state) = setup().await;
    let cp = execute(&root, &state, &input()).await;
    let ws = state.workspaces.get(&root).await.unwrap();
    let service = DaemonService::new(state.clone());
    let catalog = service
        .query_coding_workflows(Request::new(request(&root, json!({"operation":"catalog"}))))
        .await
        .unwrap()
        .into_inner();
    assert!(catalog.error_json.is_empty());
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&catalog.payload_json).unwrap()["templates"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    let invalid = service
        .query_coding_workflows(Request::new(request(
            &root,
            json!({"operation":"compile","input":input()}),
        )))
        .await
        .unwrap()
        .into_inner();
    assert!(invalid.error_json.contains("model_unavailable"));
    ws.config.write().await.llm.models.insert(
        "fixture".into(),
        metteur_shared::config::LlmModelConfig {
            api_type: "openai-chat".into(),
            model_id: "fixture".into(),
            api_endpoint: "http://127.0.0.1:1/v1".into(),
            ..Default::default()
        },
    );
    let compiled = service
        .query_coding_workflows(Request::new(request(
            &root,
            json!({"operation":"compile","input":input()}),
        )))
        .await
        .unwrap()
        .into_inner();
    assert!(compiled.error_json.is_empty(), "{}", compiled.error_json);
    let compiled: serde_json::Value = serde_json::from_str(&compiled.payload_json).unwrap();
    assert_eq!(compiled["preflight"]["executed"], false);
    assert_eq!(compiled["preflight"]["model_calls"], 0);
    assert_eq!(compiled["preflight"]["files"][0]["status"], "available");
    assert_eq!(std::fs::read_to_string(root.join("source.txt")).unwrap(), "original bytes");
    let prepared = service
        .query_coding_workflows(Request::new(request(
            &root,
            json!({"operation":"prepare_run","blueprint_id":cp.view.root.as_ref().unwrap().id}),
        )))
        .await
        .unwrap()
        .into_inner();
    assert!(prepared.error_json.is_empty(), "{}", prepared.error_json);
    let prepared: serde_json::Value = serde_json::from_str(&prepared.payload_json).unwrap();
    assert_eq!(prepared["digest"].as_str().unwrap().len(), 64);
    ws.config.write().await.llm.models.get_mut("fixture").unwrap().model_id =
        "changed-model".into();
    let changed = service
        .query_coding_workflows(Request::new(request(
            &root,
            json!({"operation":"prepare_run","blueprint_id":cp.view.root.as_ref().unwrap().id}),
        )))
        .await
        .unwrap()
        .into_inner();
    let changed: serde_json::Value = serde_json::from_str(&changed.payload_json).unwrap();
    assert_ne!(prepared["digest"], changed["digest"]);
    let rejected = service
        .execute_blueprint(Request::new(metteur_proto::proto::ExecuteBlueprintRequest {
            workspace_path: root.to_string_lossy().into_owned(),
            blueprint_id: cp.view.root.as_ref().unwrap().id.to_string(),
            blueprint_json: String::new(),
            workflow_confirmation: "outdated-confirmation".into(),
        }))
        .await
        .err()
        .expect("Stale confirmation must fail before execution");
    assert!(rejected.message().contains("Review and confirm"));
    let mut cp = cp;
    let id = cp.run_id;
    cp.view
        .invocations
        .iter_mut()
        .find(|i| cp.view.workflow_evidence.contains_key(&i.sequence))
        .unwrap()
        .outputs =
        json!({"password":"secret-value","text":"company-secret sk-abcdefghijklmnopqrstuvwxyz"});
    DbCheckpointSink::new(ws.db.clone(), id).write(&cp).unwrap();
    ws.config.write().await.anonymize.extra_patterns = vec!["company-secret".into()];
    let projected = service
        .query_coding_workflows(Request::new(request(
            &root,
            json!({"operation":"review","run_id":id}),
        )))
        .await
        .unwrap()
        .into_inner();
    assert!(projected.error_json.is_empty(), "{}", projected.error_json);
    assert!(!projected.payload_json.contains("secret-value"));
    assert!(!projected.payload_json.contains("company-secret"));
    assert!(!projected.payload_json.contains("sk-abcdefghijkl"));
    let mut forged =
        request(&root, serde_json::to_value(decision(&project(&root, &cp).unwrap())).unwrap());
    forged.request_id = Uuid::new_v4().to_string();
    let mut value: serde_json::Value = serde_json::from_str(&forged.payload_json).unwrap();
    value["subject"] = json!("other-user");
    forged.payload_json = value.to_string();
    assert!(
        service
            .record_workflow_review(Request::new(forged))
            .await
            .unwrap()
            .into_inner()
            .error_json
            .contains("invalid_request")
    );
    *state.acl_store.write().unwrap() = AclConfig {
        rules: vec![AclRule {
            subject: "local".into(),
            allow: vec!["/metteur.Daemon/QueryCodingWorkflows".into()],
            deny: vec![],
        }],
    };
    assert!(
        service
            .query_coding_workflows(Request::new(request(
                &root,
                json!({"operation":"review","run_id":id})
            )))
            .await
            .unwrap()
            .into_inner()
            .error_json
            .contains("permission_denied")
    );
    let (other, other_state) = setup().await;
    let other_service = DaemonService::new(other_state);
    assert!(
        other_service
            .query_coding_workflows(Request::new(request(
                &other,
                json!({"operation":"review","run_id":id})
            )))
            .await
            .unwrap()
            .into_inner()
            .error_json
            .contains("unavailable")
    );
    assert!(Registry::with_builtins().tool("RecordWorkflowReview").is_none());
}

#[test]
fn evidence_limits_missing_and_metadata_never_become_available() {
    let root = std::env::temp_dir().join(format!("workflow-files-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let graph =
        compile_template(&input(), &metteur_shared::node_catalog::builtin_catalog()).unwrap();
    let mut contract = read_contract(&graph).unwrap().unwrap();
    assert_eq!(capture(&root, &contract).files[0].status, "missing");
    let file = std::fs::File::create(root.join("source.txt")).unwrap();
    file.set_len(4 * 1024 * 1024 + 1).unwrap();
    assert_eq!(capture(&root, &contract).files[0].status, "size_limit");
    contract.scope_paths = vec!["../outside".into()];
    contract.evidence_paths = vec![".metteur/config".into()];
    assert!(
        capture(&root, &contract)
            .files
            .iter()
            .all(|f| f.status == "invalid_path" && f.digest.is_none())
    );
}

#[cfg(unix)]
#[test]
fn evidence_rejects_real_symbolic_links() {
    let root = std::env::temp_dir().join(format!("workflow-link-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let outside = root.with_extension("outside");
    std::fs::write(&outside, "outside scope").unwrap();
    std::os::unix::fs::symlink(&outside, root.join("source.txt")).unwrap();
    let graph =
        compile_template(&input(), &metteur_shared::node_catalog::builtin_catalog()).unwrap();
    let snapshot = capture(&root, &read_contract(&graph).unwrap().unwrap());
    assert_eq!(snapshot.files[0].status, "link_rejected");
    assert!(snapshot.files[0].digest.is_none());
}
