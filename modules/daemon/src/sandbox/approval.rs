//! Approval decisions, scoped grants and the lifetime of pending requests.

mod request;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::oneshot;
use tokio::time::Instant;

use super::grant::GrantStore;
use crate::error::{DaemonError, DaemonResult};

pub use request::PendingApproval;

/// An explicit user decision on an approval request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Permit the operation within the selected scope.
    Allow,
    /// Refuse the operation within the selected scope.
    Deny,
}

/// The scope selected by the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Only the current request.
    Once,
    /// Matching requests in this run.
    Run,
    /// Matching requests in the workspace database.
    Workspace,
    /// Matching requests in the global database.
    Global,
}

/// Parses the eight decision strings accepted by RespondApproval.
pub fn parse_decision(raw: &str) -> Option<(Decision, Scope)> {
    let (decision, rest) = if let Some(rest) = raw.strip_prefix("Allow") {
        (Decision::Allow, rest)
    } else {
        (Decision::Deny, raw.strip_prefix("Deny")?)
    };
    let scope = match rest {
        "Once" => Scope::Once,
        "Run" => Scope::Run,
        "Workspace" => Scope::Workspace,
        "Global" => Scope::Global,
        _ => return None,
    };
    Some((decision, scope))
}

/// A consumed response, preserving its identity and user-selected scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalResponse {
    /// The immutable request answered by the user.
    pub request_id: String,
    /// The user's allow or deny decision.
    pub decision: Decision,
    /// The scope applied by the broker before releasing the waiter.
    pub scope: Scope,
}

/// Failure to obtain a usable response; never a user denial or a grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ApprovalFailure {
    /// The response deadline elapsed.
    #[error("approval request expired")]
    Expired,
    /// The owning operation was cancelled.
    #[error("approval request cancelled")]
    Cancelled,
    /// The owning run ended or its response channel closed.
    #[error("approval run closed")]
    Closed,
    /// The selected persistent scope could not be stored.
    #[error("approval grant could not be persisted")]
    Persistence,
}

type Outcome = Result<ApprovalResponse, ApprovalFailure>;

#[derive(Default)]
struct BrokerState {
    closed: bool,
    pending: HashMap<String, PendingRequest>,
    run_grants: HashMap<u64, bool>,
}

struct PendingRequest {
    command_hash: u64,
    command: String,
    deadline: Instant,
    cancelled: Arc<AtomicBool>,
    tx: oneshot::Sender<Outcome>,
}

/// Owns pending requests and grants for exactly one execution or chat run.
#[derive(Default)]
pub struct ApprovalBroker {
    closed_notify: tokio::sync::Notify,
    state: Arc<Mutex<BrokerState>>,
}

impl ApprovalBroker {
    /// Creates an empty, open broker for a new run.
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens a request with immutable server-owned content and a deadline.
    ///
    /// Callers cannot supply or rebind an id. Dropping the returned handle
    /// withdraws the request even when the task is aborted before it waits.
    pub fn open_request(
        &self,
        command_hash: u64,
        command: impl Into<String>,
        timeout: Duration,
        cancelled: Arc<AtomicBool>,
    ) -> Result<PendingApproval, ApprovalFailure> {
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return Err(ApprovalFailure::Closed);
        }
        if cancelled.load(Ordering::SeqCst) {
            return Err(ApprovalFailure::Cancelled);
        }
        let deadline = Instant::now().checked_add(timeout).ok_or(ApprovalFailure::Expired)?;
        let request_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        state.pending.insert(
            request_id.clone(),
            PendingRequest {
                command_hash,
                command: command.into(),
                deadline,
                cancelled: cancelled.clone(),
                tx,
            },
        );
        Ok(PendingApproval::new(request_id, self.state.clone(), rx, deadline, cancelled))
    }

    /// Consumes one live response and applies its scope before waking the caller.
    ///
    /// This lock is also used by request withdrawal and run closure. The
    /// pending handle keeps its receiver alive until it acquires that lock,
    /// so it cannot disappear between persistence and response delivery.
    pub fn respond(
        &self,
        request_id: &str,
        decision: Decision,
        scope: Scope,
        grants: &GrantStore,
    ) -> DaemonResult<()> {
        let mut state = self.state.lock().unwrap();
        let pending = state.pending.remove(request_id).ok_or_else(|| {
            DaemonError::Sandbox("approval request expired, already answered, or unknown".into())
        })?;
        let failure = if state.closed {
            Some(ApprovalFailure::Closed)
        } else if pending.cancelled.load(Ordering::SeqCst) {
            Some(ApprovalFailure::Cancelled)
        } else if Instant::now() >= pending.deadline {
            Some(ApprovalFailure::Expired)
        } else if pending.tx.is_closed() {
            Some(ApprovalFailure::Closed)
        } else {
            None
        };
        if let Some(failure) = failure {
            let _ = pending.tx.send(Err(failure));
            return Err(DaemonError::Sandbox(failure.to_string()));
        }
        // The key and content come from the original request, never the reply.
        if let Err(error) = grants.store(scope, pending.command_hash, decision, &pending.command) {
            let _ = pending.tx.send(Err(ApprovalFailure::Persistence));
            return Err(error);
        }
        if scope == Scope::Run {
            state.run_grants.insert(pending.command_hash, decision == Decision::Allow);
        }
        let response = ApprovalResponse {
            request_id: request_id.to_string(),
            decision,
            scope,
        };
        pending
            .tx
            .send(Ok(response))
            .map_err(|_| DaemonError::Sandbox("approval response receiver unavailable".into()))
    }

    /// Caches a risk review performed under the user's existing full-mode policy.
    pub fn record_run_grant(&self, command_hash: u64, allow: bool) {
        let mut state = self.state.lock().unwrap();
        if !state.closed {
            state.run_grants.insert(command_hash, allow);
        }
    }

    /// Looks up a matching decision within this broker's run only.
    pub fn run_grant(&self, command_hash: u64) -> Option<bool> {
        self.state.lock().unwrap().run_grants.get(&command_hash).copied()
    }

    /// Whether the owning run has ended and can no longer authorize actions.
    pub fn is_closed(&self) -> bool {
        self.state.lock().unwrap().closed
    }

    /// Closes the run permanently without turning cancellation into user Deny.
    pub fn close(&self) {
        let mut state = self.state.lock().unwrap();
        state.closed = true;
        state.run_grants.clear();
        self.closed_notify.notify_waiters();
        for (_, pending) in state.pending.drain() {
            let _ = pending.tx.send(Err(ApprovalFailure::Closed));
        }
    }

    /// Ensures task abortion or unwinding also closes the run's approvals.
    pub fn close_on_drop(self: &Arc<Self>) -> ApprovalRunGuard {
        ApprovalRunGuard(self.clone())
    }

    /// Event-driven run closure, including closure before the waiter is registered.
    pub async fn closed(&self) {
        let notified=self.closed_notify.notified();tokio::pin!(notified);notified.as_mut().enable();
        if !self.is_closed() {notified.await;}
    }
    /// Returns unanswered request ids for diagnostics and tests.
    pub fn pending_ids(&self) -> Vec<String> {
        self.state.lock().unwrap().pending.keys().cloned().collect()
    }
}

/// Closes a run's broker when its owning task exits.
pub struct ApprovalRunGuard(Arc<ApprovalBroker>);

impl Drop for ApprovalRunGuard {
    fn drop(&mut self) {
        self.0.close();
    }
}
