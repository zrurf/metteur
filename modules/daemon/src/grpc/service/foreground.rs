//! A read-only snapshot of the same foreground owners used by SendChat.
use super::DaemonService;
use crate::storage::resources::{Result, fault};
use serde::Serialize;
use std::path::Path;
use uuid::Uuid;

#[derive(Serialize)]
pub(crate) struct Foreground {
    pub state: &'static str,
    pub run: Option<Uuid>,
    pub session: Option<Uuid>,
    pub turn: Option<Uuid>,
    pub reason: &'static str,
}
pub(crate) fn unresolved_writes(
    db: &crate::storage::persistence::Db,
    root: &Path,
) -> crate::error::DaemonResult<bool> {
    let tasks = crate::continuation::store::ContinuationStore::inspect(db.clone(), root)?;
    let mut after = None;
    loop {
        let page = tasks.list(after)?;
        if page.is_empty() {
            return Ok(false);
        }
        for record in page {
            after = Some(record.task.id);
            if record.task.sources.values().any(|source| {
                source.may_write
                    && (source.terminal.is_none()
                        || matches!(
                            source.terminal,
                            Some(
                                crate::continuation::state::Terminal::Indeterminate
                                    | crate::continuation::state::Terminal::Interrupted
                                    | crate::continuation::state::Terminal::Cancelled
                            )
                        ))
            }) {
                return Ok(true);
            }
        }
    }
}

impl DaemonService {
    /// Owner lookup for a concrete control, without waiting for file writes.
    /// Its idle result is not an authorization to start a new turn.
    pub(crate) async fn control_target(&self, root: &Path) -> Foreground {
        let mut state = Foreground {
            state: "idle",
            run: None,
            session: None,
            turn: None,
            reason: "",
        };
        if let Some(run) = self.state.running.read().await.get(root) {
            state.state = "blueprint";
            state.run = Some(run.run_id);
            state.reason = if run.cancel_requested.load(std::sync::atomic::Ordering::SeqCst) {
                "Blueprint is stopping"
            } else if run.pause_requested.load(std::sync::atomic::Ordering::SeqCst) {
                "Blueprint is paused"
            } else if !run.approval_details.lock().is_empty() {
                "Blueprint is waiting for approval"
            } else {
                "Blueprint is running"
            };
            return state;
        }
        if let Some(chat) = self.state.chats.read().await.get(root) {
            state.state = "chat";
            state.session = chat.session_id;
            state.turn = Some(chat.turn_id);
            state.reason = if chat.cancel_requested.load(std::sync::atomic::Ordering::SeqCst) {
                "Chat is stopping"
            } else if !chat.approval_details.lock().is_empty() {
                "Chat is waiting for approval"
            } else {
                "Chat is running"
            };
            return state;
        }
        state
    }
    /// The caller holds admission to combine live ownership and write effects.
    pub(crate) async fn foreground(
        &self,
        root: &Path,
        db: &crate::storage::persistence::Db,
    ) -> Result<Foreground> {
        let mut state = self.control_target(root).await;
        if state.state != "idle" {
            return Ok(state);
        }
        if unresolved_writes(db, root).map_err(|_| {
            fault("storage_unavailable", "Foreground write ownership could not be checked")
        })? {
            state.state = "blocked";
            state.reason = "A background write effect needs reconciliation";
        }
        Ok(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn availability_read_does_not_migrate_or_notify_and_unknown_schema_fails_closed() {
        let root = std::env::temp_dir().join(format!("dw-foreground-read-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let db = crate::storage::persistence::Db::open(&root.join(".metteur/db")).unwrap();
        let changes = db.subscribe_changes();
        assert_eq!(crate::storage::resources::ResourceStore::inspect(&db).unwrap(), None);
        assert!(!unresolved_writes(&db, &root).unwrap());
        assert_eq!(crate::storage::resources::ResourceStore::inspect(&db).unwrap(), None);
        assert!(!root.join(".metteur/nx-before-v1").exists());
        assert!(!changes.has_changed().unwrap());
        db.put_durable(
            crate::storage::persistence::cf::EXECUTION_STATE,
            b"nx/resources/schema",
            b"2",
        )
        .unwrap();
        assert!(unresolved_writes(&db, &root).is_err());
    }
}
