//! Confirmed model control requests. Direct user controls do not enter this path.
use super::{actions, requests::State, diagnostic::{self, Category, Stage}};
use crate::{
    DaemonError, DaemonResult,
    execution::{
        DbCheckpointSink, RunStatus,
        context::ExecutionContext,
        file_journal::{DbFileJournal, FileJournalStore},
        transaction::TransactionEntry,
    },
};
use serde_json::{Value, json};
use std::sync::atomic::Ordering;
use uuid::Uuid;
fn error(s: &str) -> DaemonError {
    DaemonError::Execution(s.into())
}
fn snapshot(ctx: &ExecutionContext) -> DaemonResult<Value> {
    actions::enabled(ctx)?;
    let db = ctx.workspace_db.as_ref().ok_or_else(|| error("Workspace unavailable"))?;
    let cp = DbCheckpointSink::load(db, ctx.run_id)?.ok_or_else(|| error("Run unavailable"))?;
    if cp.status != RunStatus::Running {
        return Err(error("Run is not active"));
    }
    let config = ctx
        .config
        .as_ref()
        .ok_or_else(|| error("Configuration unavailable"))?
        .try_read()
        .map_err(|_| error("Configuration changing"))?;
    let mut scope = Vec::new();
    let operations = DbFileJournal(db.clone()).operations()?;
    for entry in ctx.transaction_log.entries() {
        match entry {
            TransactionEntry::FileWrite {
                path,
                reverted: false,
                ..
            }
            | TransactionEntry::FileDelete {
                path,
                reverted: false,
                ..
            } => scope.push(json!({"path":path})),
            TransactionEntry::FileOperation {
                operation_id,
                ..
            } => {
                if let Some(op) = operations.iter().find(|op| op.operation_id == operation_id) {
                    scope
                        .push(json!({"operation_id":operation_id,"path":op.path,"phase":op.phase}));
                }
            }
            _ => {}
        }
    }
    Ok(
        json!({"base":cp.blueprint_version,"in_flight":cp.in_flight,"executed":cp.executed,"pending":cp.pending,"pause_requested":ctx.pause_requested.load(Ordering::SeqCst),"rollback_on_cancel":config.execution.rollback_on_cancel,"recorded_file_scope":scope}),
    )
}
pub(crate) async fn propose(
    ctx: &mut ExecutionContext,
    review: Uuid,
    kind: &str,
    summary: &str,
) -> DaemonResult<Value> {
    if !matches!(kind, "PauseRun" | "CancelRun") {
        return Err(error("Unknown control action"));
    }
    let before = snapshot(ctx)?;
    if kind == "PauseRun" && ctx.pause_requested.load(Ordering::SeqCst) {
        return Err(error("Pause already requested"));
    }
    let id = Uuid::new_v4();
    let mut guard = actions::register(
        ctx,
        review,
        id,
        kind,
        json!({"request_type":"oversight_control","tool":kind,"summary":summary,"dangerous":kind=="CancelRun","expected":before,"rollback_notice":"Only recorded file mutations are eligible for rollback. Approved blueprint changes and shell/network effects are retained. In-flight file operations may extend this scope; conflicts will be reported after cancellation."}),
    )?;
    if !guard.confirm(ctx).await? {
        return Err(diagnostic::error(Category::ApprovalRejected, Stage::Approval));
    }
    let db = ctx.workspace_db.as_ref().ok_or_else(|| error("Workspace unavailable"))?;
    let flag = if kind == "CancelRun" {
        &ctx.cancel_requested
    } else {
        &ctx.pause_requested
    };
    let result = actions::apply_control(db, ctx.run_id, id, kind, || {
        actions::authorize_application(ctx, id)?;
        if snapshot(ctx)? != before {
            return Err(diagnostic::error(Category::VersionStale, Stage::VersionCheck));
        }
        flag.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| error("Control was already requested"))?;
        Ok(())
    });
    if let Err(error) = result {
        guard.failed(&error, Stage::Application)?;
        return Err(error);
    }
    guard.staged();
    if kind == "CancelRun"
        && let Some(broker) = &ctx.approvals
    {
        broker.close();
    }
    ctx.audit(
        "oversight.control_applied",
        json!({"proposal_id":id,"run_id":ctx.run_id,"kind":kind}),
    );
    Ok(
        json!({"proposal_id":id,"state":State::Applied,"control_requested":kind,"rollback_complete":false}),
    )
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CancelResult {
    pub rollback_requested: bool,
    pub restored_operations: Option<usize>,
    pub error: Option<String>,
    pub files: Vec<Value>,
    pub at_ms: u64,
}
pub(crate) fn record_result(
    ctx: &ExecutionContext,
    rollback_requested: bool,
    restored_operations: Option<usize>,
    error: Option<String>,
) -> DaemonResult<()> {
    let Some(db) = &ctx.workspace_db else {
        return Ok(());
    };
    let operations = DbFileJournal(db.clone()).operations()?;
    let mut files = Vec::new();
    for entry in ctx.transaction_log.entries() {
        match entry {
            TransactionEntry::FileWrite {
                path,
                reverted,
                ..
            }
            | TransactionEntry::FileDelete {
                path,
                reverted,
                ..
            } => files
                .push(json!({"path":path,"phase":if reverted {"Reverted"}else{"Not reverted"}})),
            TransactionEntry::FileOperation {
                operation_id,
                ..
            } => {
                if let Some(op) = operations.iter().find(|op| op.operation_id == operation_id) {
                    files
                        .push(json!({"path":op.path,"phase":op.phase,"operation_id":operation_id}));
                }
            }
            _ => {}
        }
    }
    let _gate = db
        .oversight_gate
        .lock()
        .map_err(|_| crate::DaemonError::Persistence("Oversight lock poisoned".into()))?;
    let Some(mut schedule) = super::scheduler::load(db, ctx.run_id)? else {
        return Ok(());
    };
    // The trusted interpreter may finish rollback after model work was cancelled.
    // Updating its outcome never reopens the scheduler or authorizes another action.
    schedule.cancel_result = Some(CancelResult {
        rollback_requested,
        restored_operations,
        error,
        files,
        at_ms: super::scheduler::now(),
    });
    super::scheduler::save(db, ctx.run_id, &schedule)
}
