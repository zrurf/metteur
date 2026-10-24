use super::*;
use crate::sandbox::approval::{Decision, PendingApproval, Scope};
use crate::{AppState, Registry, WorkspaceManager};
use std::{
    collections::HashMap,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
struct Fixture {
    service: DaemonService,
    state: Arc<AppState>,
    ws: Arc<crate::workspace::manager::Workspace>,
    broker: Arc<ApprovalBroker>,
    pending: PendingApproval,
    target: Target,
    digest: String,
}
async fn setup() -> Fixture {
    let root = std::env::temp_dir().join(format!("dw-approval-rpc-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let state = Arc::new(AppState::new(
        WorkspaceManager::new().with_global_config_path(root.join("global.toml")),
        Arc::new(Registry::with_builtins()),
        Default::default(),
    ));
    let ws = state.workspaces.open(&root).await.unwrap();
    let broker = Arc::new(ApprovalBroker::new());
    let pending = broker
        .open_request(
            7,
            "echo immutable",
            Duration::from_secs(60),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
    let detail = r#"{"command":"echo immutable","outside_workspace":false}"#.to_string();
    let target = Target {
        kind: "chat".into(),
        session: Some(Uuid::new_v4()),
        turn: Some(Uuid::new_v4()),
        run: None,
    };
    state.chats.write().await.insert(
        ws.root.clone(),
        super::super::ChatRun {
            continuation_task: None,
            transcript: Arc::new(parking_lot::Mutex::new(
                crate::chat::transcript::Transcript::resuming(vec![]),
            )),
            turn_id: target.turn.unwrap(),
            approval_details: Arc::new(parking_lot::Mutex::new(HashMap::from([(
                pending.id().to_string(),
                detail.clone(),
            )]))),
            interrupt_bus: None,
            cancel_requested: Arc::new(AtomicBool::new(false)),
            approvals: Some(broker.clone()),
            persist: Arc::new(AtomicBool::new(true)),
            session_id: target.session,
        },
    );
    Fixture {
        service: DaemonService::new(state.clone()),
        state,
        ws,
        broker,
        pending,
        target,
        digest: ApprovalDelivery::content_digest(&detail),
    }
}
fn decision(f: &Fixture, choice: &str) -> ApprovalDecisionRequest {
    ApprovalDecisionRequest {
        workspace_path: f.ws.root.to_string_lossy().into(),
        request_id: f.pending.id().into(),
        decision: choice.into(),
        target_json: serde_json::to_string(&f.target).unwrap(),
        content_digest: f.digest.clone(),
    }
}
async fn query(f: &Fixture, action: serde_json::Value) -> ResourceResponse {
    f.service
        .approval_delivery(Request::new(ResourceRequest {
            schema_version: 1,
            workspace_path: f.ws.root.to_string_lossy().into(),
            payload_json: action.to_string(),
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner()
}
fn body(response: ResourceResponse) -> serde_json::Value {
    assert!(response.error_json.is_empty(), "{}", response.error_json);
    serde_json::from_str(&response.payload_json).unwrap()
}
#[tokio::test]
async fn fixed_decision_is_durable_and_duplicate_cannot_regrant_or_escalate() {
    let f = setup().await;
    let id = f.pending.id().to_string();
    let command = decision(&f, "AllowOnce");
    f.service.respond_approval(Request::new(command.clone())).await.unwrap();
    f.service.respond_approval(Request::new(command)).await.unwrap();
    assert!(f.service.respond_approval(Request::new(decision(&f, "AllowGlobal"))).await.is_err());
    let data = body(query(&f, serde_json::json!({"operation":"read","id":id})).await);
    assert_eq!(data["receipt"]["phase"], "accepted");
    assert_eq!(data["receipt"]["decision"], "AllowOnce");
    assert!(f.broker.pending_ids().is_empty());
    let result = f.pending.wait().await.unwrap();
    assert_eq!(result.decision, Decision::Allow);
    assert_eq!(result.scope, Scope::Once);
}
#[tokio::test]
async fn target_and_content_mismatch_keep_the_original_pending_request() {
    let f = setup().await;
    let mut wrong = decision(&f, "AllowOnce");
    wrong.content_digest = ApprovalDelivery::content_digest("different content");
    assert!(f.service.respond_approval(Request::new(wrong)).await.is_err());
    let retired = query(&f, serde_json::json!({"operation":"read_or_retire","id":f.pending.id(),"target":f.target,"content_digest":ApprovalDelivery::content_digest("not the shown action")})).await;
    assert!(retired.error_json.contains("identity_conflict"));
    let mut wrong = decision(&f, "AllowOnce");
    let mut target = f.target.clone();
    target.turn = Some(Uuid::new_v4());
    wrong.target_json = serde_json::to_string(&target).unwrap();
    assert!(f.service.respond_approval(Request::new(wrong)).await.is_err());
    assert!(
        ApprovalDelivery::new(f.ws.db.clone(), "local")
            .unwrap()
            .read(Uuid::parse_str(f.pending.id()).unwrap())
            .unwrap()
            .is_none()
    );
    assert_eq!(f.broker.pending_ids(), vec![f.pending.id().to_string()]);
    f.service.respond_approval(Request::new(decision(&f, "DenyOnce"))).await.unwrap();
    assert_eq!(f.pending.wait().await.unwrap().decision, Decision::Deny);
}
#[tokio::test]
async fn read_and_retirement_do_not_wait_for_file_admission_and_fence_legacy_late_responses() {
    let f = setup().await;
    let id = f.pending.id().to_string();
    let _file_writer = f.ws.activity_gate.lock().await;
    let described = body(
        tokio::time::timeout(
            Duration::from_secs(1),
            query(&f, serde_json::json!({"operation":"describe","id":id})),
        )
        .await
        .unwrap(),
    );
    assert_eq!(described["pending"]["content_digest"], f.digest);
    let retired = body(query(&f, serde_json::json!({"operation":"read_or_retire","id":id,"target":f.target,"content_digest":f.digest})).await);
    assert_eq!(retired["receipt"]["phase"], "retired");
    let mut legacy = decision(&f, "AllowGlobal");
    legacy.target_json.clear();
    legacy.content_digest.clear();
    assert!(f.service.respond_approval(Request::new(legacy)).await.is_err());
    assert!(f.pending.wait().await.is_err());
    assert!(GrantStore::new(Some(f.ws.db.clone()), None).lookup(7).is_none());
}
#[tokio::test]
async fn failed_claim_wal_does_not_touch_the_broker_or_grants() {
    let f = setup().await;
    f.ws.db.fail_delivery_write.store(true, Ordering::SeqCst);
    assert!(
        f.service.respond_approval(Request::new(decision(&f, "AllowWorkspace"))).await.is_err()
    );
    assert_eq!(f.broker.pending_ids(), vec![f.pending.id().to_string()]);
    assert!(
        ApprovalDelivery::new(f.ws.db.clone(), "local")
            .unwrap()
            .read(Uuid::parse_str(f.pending.id()).unwrap())
            .unwrap()
            .is_none()
    );
    assert!(GrantStore::new(Some(f.ws.db.clone()), None).lookup(7).is_none());
}
#[tokio::test]
async fn grant_failure_is_unknown_and_never_retried_after_receipt_reconciliation() {
    let f = setup().await;
    let id = f.pending.id().to_string();
    assert!(f.service.respond_approval(Request::new(decision(&f, "AllowGlobal"))).await.is_err());
    assert_eq!(
        body(query(&f, serde_json::json!({"operation":"read","id":id})).await)["receipt"]["phase"],
        "unknown"
    );
    assert!(f.service.respond_approval(Request::new(decision(&f, "AllowGlobal"))).await.is_err());
    assert!(f.pending.wait().await.is_err());
}
#[tokio::test]
async fn crash_window_after_broker_response_has_claimed_receipt_and_cannot_repeat_effect() {
    let f = setup().await;
    let id = Uuid::parse_str(f.pending.id()).unwrap();
    let store = ApprovalDelivery::new(f.ws.db.clone(), "local").unwrap();
    store.claim(id, &f.target, &f.digest, "AllowWorkspace").unwrap();
    let grants = GrantStore::new(Some(f.ws.db.clone()), None);
    f.broker.respond(f.pending.id(), Decision::Allow, Scope::Workspace, &grants).unwrap();
    // Simulate loss before the final receipt sync; a retry only reads Claimed.
    assert!(
        f.service.respond_approval(Request::new(decision(&f, "AllowWorkspace"))).await.is_err()
    );
    assert_eq!(
        body(query(&f, serde_json::json!({"operation":"read","id":id})).await)["receipt"]["phase"],
        "claimed"
    );
    assert_eq!(grants.lookup(7), Some(true));
    assert_eq!(f.pending.wait().await.unwrap().scope, Scope::Workspace);
}
#[tokio::test]
async fn blueprint_target_is_distinct_and_receipt_capability_respects_current_acl() {
    let mut f = setup().await;
    let id = f.pending.id().to_string();
    f.state.chats.write().await.clear();
    let detail = r#"{"command":"echo immutable","outside_workspace":false}"#.to_string();
    f.target = Target {
        kind: "blueprint".into(),
        session: None,
        turn: None,
        run: Some(Uuid::new_v4()),
    };
    f.state.running.write().await.insert(
        f.ws.root.clone(),
        super::super::RunningExecution {
            approval_details: Arc::new(parking_lot::Mutex::new(HashMap::from([(
                id.clone(),
                detail,
            )]))),
            blueprint_id: Uuid::new_v4(),
            continuation_task: None,
            run_id: f.target.run.unwrap(),
            interrupt_bus: None,
            pause_requested: Arc::new(AtomicBool::new(false)),
            cancel_requested: Arc::new(AtomicBool::new(false)),
            approvals: Some(f.broker.clone()),
        },
    );
    f.service.respond_approval(Request::new(decision(&f, "DenyRun"))).await.unwrap();
    assert_eq!(f.broker.run_grant(7), Some(false));
    *f.state.acl_store.write().unwrap() = metteur_shared::config::AclConfig {
        rules: vec![metteur_shared::config::AclRule {
            subject: "local".into(),
            allow: vec!["/metteur.Daemon/ListWorkspaces".into()],
            deny: vec![],
        }],
    };
    assert!(
        query(&f, serde_json::json!({"operation":"capabilities"}))
            .await
            .error_json
            .contains("permission_denied")
    );
    assert!(f.pending.wait().await.is_ok());
}
