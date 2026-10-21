//! Host-owned turn lifetime. A browser stream observes a registered task;
//! it does not own it. Ordinary chats retain disconnect cancellation.

use crate::sandbox::approval::ApprovalBroker;
use std::{
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObserverPolicy {
    CancelOnDisconnect,
    /// Only the host may select this after a durable task policy is validated.
    DetachedTask,
}

pub async fn run_observed<T>(
    run: impl Future<Output = T>,
    observer_closed: impl Future<Output = ()>,
    policy: ObserverPolicy,
    cancel: Arc<AtomicBool>,
    approvals: Arc<ApprovalBroker>,
) -> T {
    tokio::pin!(run);
    tokio::select! {
        biased;
        _ = observer_closed, if policy == ObserverPolicy::CancelOnDisconnect => {
            cancel.store(true, Ordering::SeqCst);
            approvals.close();
            // Drain the kernel so its partial context and tool results survive.
            run.await
        }
        result = &mut run => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn ordinary_disconnect_cancels_and_drains_partial_result() {
        let cancel = Arc::new(AtomicBool::new(false));
        let seen = cancel.clone();
        let result = run_observed(
            async move {
                assert!(seen.load(Ordering::SeqCst));
                "partial context"
            },
            async {},
            ObserverPolicy::CancelOnDisconnect,
            cancel,
            Arc::new(ApprovalBroker::new()),
        )
        .await;
        assert_eq!(result, "partial context");
    }
    #[tokio::test]
    async fn detached_observer_cannot_cancel_host_turn() {
        let cancel = Arc::new(AtomicBool::new(false));
        let result = run_observed(
            async { "saved output" },
            async {},
            ObserverPolicy::DetachedTask,
            cancel.clone(),
            Arc::new(ApprovalBroker::new()),
        )
        .await;
        assert_eq!(result, "saved output");
        assert!(!cancel.load(Ordering::SeqCst));
    }
}
