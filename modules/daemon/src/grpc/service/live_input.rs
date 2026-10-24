//! Reliable Queue/Steer use the existing root chat bus, never a second runner.
use super::{DaemonService, state::ChatRun};
use crate::{
    DaemonError,
    chat::transcript::Transcript,
    execution::interrupt::{Interrupt, InterruptPriority, OwnedInterrupt},
    storage::{
        delivery::{DeliveryPhase, DeliveryReceipt, DeliveryStore, LiveAcceptance},
        resources::{Result, digest, fault},
    },
    workspace::manager::Workspace,
};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LiveInput {
    pub id: Uuid,
    pub session: Uuid,
    pub turn: Uuid,
    pub mode: String,
    pub message: String,
    #[serde(default)]
    pub prompt_snapshots: Vec<Uuid>,
    #[serde(default)]
    pub file_references: Vec<super::file_references::FileReference>,
    #[serde(default)]
    pub result_references: Vec<super::result_references::ResultReference>,
    #[serde(default)]
    pub context_order: Vec<super::file_references::ContextOrder>,
}
fn time() -> u64 {
    chrono::Utc::now().timestamp_millis() as u64
}
fn failure(error: impl std::fmt::Display) -> DaemonError {
    DaemonError::Persistence(error.to_string())
}
struct InputOwner {
    store: DeliveryStore,
    id: Uuid,
    body: String,
    journal: Arc<parking_lot::Mutex<Transcript>>,
}
impl OwnedInterrupt for InputOwner {
    fn claim(&self) -> crate::error::DaemonResult<()> {
        self.store.transition_live(self.id, DeliveryPhase::LiveClaimed, time()).map_err(failure)?;
        self.journal.lock().push_user(self.body.clone(), time());
        Ok(())
    }
    fn unconsumed(&self) -> crate::error::DaemonResult<()> {
        self.store
            .transition_live(self.id, DeliveryPhase::LiveUnconsumed, time())
            .map_err(failure)?;
        Ok(())
    }
    fn applied(&self, persisted: bool) -> crate::error::DaemonResult<()> {
        self.store
            .transition_live(
                self.id,
                if persisted {
                    DeliveryPhase::LiveApplied
                } else {
                    DeliveryPhase::LiveUnknown
                },
                time(),
            )
            .map_err(failure)?;
        Ok(())
    }
}
impl DaemonService {
    pub(super) async fn receive_live_input(
        &self,
        ws: &Workspace,
        subject: &str,
        input: LiveInput,
    ) -> Result<DeliveryReceipt> {
        if !self
            .state
            .acl_store
            .read()
            .unwrap()
            .is_allowed(subject, "/metteur.Daemon/SendInterrupt")
        {
            return Err(fault("permission_denied", "Current input permission is required"));
        }
        if input.id.is_nil()
            || input.session.is_nil()
            || input.turn.is_nil()
            || !["queue", "steer"].contains(&input.mode.as_str())
            || input.message.trim().is_empty()
            || input.prompt_snapshots.len() > 16
        {
            return Err(fault(
                "invalid_input",
                "Queue or Steer requires a concrete turn and input",
            ));
        }
        let fingerprint = digest(
            &serde_json::to_vec(&(
                &input.session,
                &input.turn,
                &input.mode,
                &input.message,
                &input.prompt_snapshots,
                &input.file_references,
                &input.result_references,
                &input.context_order,
            ))
            .map_err(failure)
            .map_err(|e| fault("invalid_input", &e.to_string()))?,
        );
        let store = DeliveryStore::new(ws.db.clone(), &ws.root().to_string_lossy(), subject)?;
        // Look up before checking a live target. Exact retransmission after
        // closure only reads the original receipt; it never queues again.
        if let Some(receipt) = store.lookup(input.id, &fingerprint)? {
            if receipt.live.is_none() {
                return Err(fault("request_conflict", "Identity belongs to another input channel"));
            }
            return Ok(receipt);
        }
        let mut body = String::new();
        let mut prompt_parts = Vec::new();
        for id in &input.prompt_snapshots {
            let snapshot =
                self.selected_prompt(&ws.root().to_string_lossy(), subject, *id).await.map_err(
                    |_| fault("permission_denied", "Selected prompt is unavailable or changed"),
                )?;
            if snapshot.purpose != metteur_shared::resources::PromptPurpose::Chat {
                return Err(fault("permission_denied", "Selection is not for chat"));
            }
            body.push_str(&crate::prompt_snapshots::user_text(&snapshot));
            body.push_str("\n\n");
            prompt_parts.push(format!("{}\n\n", crate::prompt_snapshots::user_text(&snapshot)));
        }
        if body.len() > 64 * 1024 {
            return Err(fault("invalid_input", "Selected prompt budget exceeded"));
        }
        let files = self
            .selected_file_parts(ws, subject, &input.file_references)
            .map_err(|error| fault("invalid_file_reference", &error.to_string()))?;
        let results = self
            .selected_result_parts(ws, subject, &input.result_references)
            .map_err(|error| fault("invalid_result_reference", &error.to_string()))?;
        body = super::file_references::ordered_context(
            &prompt_parts,
            &files,
            &results,
            &input.context_order,
        )
        .map_err(|error| fault("invalid_file_reference", &error.to_string()))?;
        body.push_str(&input.message);
        if body.len() > crate::storage::delivery::MAX_LIVE_BODY {
            return Err(fault("invalid_input", "Input exceeds the bounded body limit"));
        }
        let chats = self.state.chats.read().await;
        let target = chats
            .get(ws.root())
            .filter(|target| {
                target.session_id == Some(input.session) && target.turn_id == input.turn
            })
            .ok_or_else(|| {
                fault("target_changed", "The targeted chat has ended; retain the draft")
            })?;
        if target.cancel_requested.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(fault(
                "target_stopping",
                "The targeted chat is stopping; retain the draft",
            ));
        }
        let bus = target
            .interrupt_bus
            .as_ref()
            .ok_or_else(|| fault("target_changed", "The targeted chat cannot receive input"))?;
        let owner = Arc::new(InputOwner {
            store: store.clone(),
            id: input.id,
            body: body.clone(),
            journal: target.transcript.clone(),
        });
        bus.send_owned(
            Interrupt {
                priority: if input.mode == "queue" {
                    InterruptPriority::Normal
                } else {
                    InterruptPriority::Urgent
                },
                message: body.clone(),
            },
            owner,
            || {
                store
                    .accept_live(LiveAcceptance {
                        id: input.id,
                        input_digest: &fingerprint,
                        session: input.session,
                        turn: input.turn,
                        mode: &input.mode,
                        body: &body,
                        now: time(),
                    })
                    .map_err(failure)
            },
        )
        .map_err(|error| fault("input_unconfirmed", &error.to_string()))
    }
}
/// A dead owner is never resumed from receipt data. A claimed input may have
/// reached the provider before a crash, so its only safe disposition is unknown.
pub(super) fn normalize(
    store: &DeliveryStore,
    receipt: DeliveryReceipt,
    current: Option<&ChatRun>,
) -> Result<DeliveryReceipt> {
    if current
        .is_some_and(|run| run.turn_id == receipt.turn && run.session_id == Some(receipt.session))
    {
        return Ok(receipt);
    }
    let phase = match receipt.phase {
        DeliveryPhase::LiveQueued => DeliveryPhase::LiveUnconsumed,
        DeliveryPhase::LiveClaimed => DeliveryPhase::LiveUnknown,
        _ => return Ok(receipt),
    };
    store.transition_live(receipt.id, phase, time())
}
