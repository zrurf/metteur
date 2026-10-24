//! Durable manual-input acceptance. This is a receipt ledger, not an executor.
use super::{
    persistence::{Db, cf},
    resources::{Result, digest, fault},
};
use crate::{chat::session::ChatSessionRecord, continuation::store::TaskHandle};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
mod live;
pub use live::{LiveAcceptance, LiveMetadata, MAX_LIVE_BODY};

pub const MAX_RECEIPTS: usize = 8192;
pub const MAX_RECEIPT_BYTES: usize = 4096;
pub const MAX_LEDGER_BYTES: usize = 16 * 1024 * 1024;

fn unavailable(_: impl std::fmt::Display) -> metteur_shared::resources::ResourceError {
    fault(
        "storage_unavailable",
        "Delivery receipt storage is unavailable; reconcile before retrying",
    )
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryIdentity {
    pub id: Uuid,
    pub expected_revision: u64,
    #[serde(default)]
    pub new_session: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryPhase {
    Retired,
    Accepted,
    Started,
    Finished,
    NeedsAttention,
    LiveQueued,
    LiveClaimed,
    LiveApplied,
    LiveUnconsumed,
    LiveUnknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryReceipt {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live: Option<LiveMetadata>,
    pub schema_version: u32,
    pub id: Uuid,
    pub input_digest: String,
    pub session: Uuid,
    pub turn: Uuid,
    pub checkpoint: String,
    pub accepted_revision: u64,
    pub phase: DeliveryPhase,
    pub outcome: Option<String>,
    pub accepted_ms: u64,
    pub updated_ms: u64,
}

pub struct Acceptance<'a> {
    pub identity: &'a DeliveryIdentity,
    pub input_digest: &'a str,
    pub context: &'a ChatSessionRecord,
    pub turn: Uuid,
    pub checkpoint: &'a str,
    pub task: Option<&'a TaskHandle>,
    pub now: u64,
}

#[derive(Clone)]
pub struct DeliveryStore {
    db: Db,
    prefix: String,
}
impl DeliveryStore {
    pub fn new(db: Db, workspace: &str, subject: &str) -> Result<Self> {
        if workspace.is_empty() || subject.is_empty() {
            return Err(fault("invalid_scope", "Host delivery scope is required"));
        }
        Ok(Self {
            db,
            prefix: format!(
                "dw/delivery/{}/",
                digest(&serde_json::to_vec(&(workspace, subject)).map_err(unavailable)?)
            ),
        })
    }
    fn marker(&self) -> Vec<u8> {
        format!("{}schema", self.prefix).into_bytes()
    }
    fn records_prefix(&self) -> Vec<u8> {
        format!("{}receipts/", self.prefix).into_bytes()
    }
    fn key(&self, id: Uuid) -> Vec<u8> {
        format!("{}receipts/{id}", self.prefix).into_bytes()
    }
    fn check_schema(&self) -> Result<()> {
        if self
            .db
            .get(cf::EXECUTION_STATE, &self.marker())
            .map_err(unavailable)?
            .is_some_and(|raw| raw != b"1" && raw != b"2")
        {
            return Err(fault(
                "unsupported_schema",
                "Delivery ledger schema is unsupported; no input was started",
            ));
        }
        Ok(())
    }
    fn decode(raw: &[u8]) -> Result<DeliveryReceipt> {
        let receipt: DeliveryReceipt = serde_json::from_slice(raw).map_err(|_| {
            fault("corrupt_receipt", "Delivery receipt requires review; do not replay the input")
        })?;
        if receipt.schema_version != 1 {
            return Err(fault(
                "unsupported_schema",
                "Unknown delivery receipt schema; do not replay the input",
            ));
        }
        let live_phase = matches!(
            receipt.phase,
            DeliveryPhase::LiveQueued
                | DeliveryPhase::LiveClaimed
                | DeliveryPhase::LiveApplied
                | DeliveryPhase::LiveUnconsumed
                | DeliveryPhase::LiveUnknown
        );
        if receipt.live.is_some() != live_phase
            || receipt.live.as_ref().is_some_and(|live| {
                !["queue", "steer"].contains(&live.mode.as_str()) || live.body_digest.len() != 64
            })
        {
            return Err(fault(
                "corrupt_receipt",
                "Receipt channel or live input metadata is invalid; do not replay",
            ));
        }
        Ok(receipt)
    }
    pub fn get(&self, id: Uuid) -> Result<Option<DeliveryReceipt>> {
        self.check_schema()?;
        self.db
            .get(cf::EXECUTION_STATE, &self.key(id))
            .map_err(unavailable)?
            .map(|raw| Self::decode(&raw))
            .transpose()
    }
    pub fn lookup(&self, id: Uuid, input_digest: &str) -> Result<Option<DeliveryReceipt>> {
        let receipt = self.get(id)?;
        if receipt.as_ref().is_some_and(|r| r.phase == DeliveryPhase::Retired) {
            return Err(fault(
                "delivery_retired",
                "This unaccepted delivery ID was closed. Its late input cannot start a turn.",
            ));
        }
        if receipt.as_ref().is_some_and(|r| r.input_digest != input_digest) {
            return Err(fault(
                "request_conflict",
                "Delivery ID already belongs to different input; original receipt retained",
            ));
        }
        Ok(receipt)
    }
    pub fn list(&self, after: Option<Uuid>) -> Result<Vec<DeliveryReceipt>> {
        self.check_schema()?;
        self.db
            .resource_scan(
                &self.records_prefix(),
                &after.map(|id| self.key(id)).unwrap_or_default(),
                50,
            )
            .map_err(unavailable)?
            .into_iter()
            .map(|(_, raw)| Self::decode(&raw))
            .collect()
    }
    /// A missing query alone cannot prove a delayed request will not arrive.
    /// Retire an unaccepted ID under the same lock as accept before minting a
    /// replacement. If acceptance won, return that original receipt unchanged.
    pub fn retire(&self, id: Uuid, now: u64) -> Result<DeliveryReceipt> {
        let _guard = self.db.resource_gate.lock().map_err(unavailable)?;
        if let Some(receipt) = self.get(id)? {
            return Ok(receipt);
        }
        let receipt = DeliveryReceipt {
            live: None,
            schema_version: 1,
            id,
            input_digest: String::new(),
            session: Uuid::nil(),
            turn: Uuid::nil(),
            checkpoint: String::new(),
            accepted_revision: 0,
            phase: DeliveryPhase::Retired,
            outcome: Some("not_received".into()),
            accepted_ms: 0,
            updated_ms: now,
        };
        let bytes = serde_json::to_vec(&receipt).map_err(unavailable)?;
        let rows = self
            .db
            .resource_scan(&self.records_prefix(), &[], MAX_RECEIPTS + 1)
            .map_err(unavailable)?;
        if rows.len() >= MAX_RECEIPTS
            || rows
                .iter()
                .map(|(_, raw)| raw.len())
                .sum::<usize>()
                .saturating_add(self.live_body_bytes()?)
                .saturating_add(bytes.len())
                > MAX_LEDGER_BYTES
        {
            return Err(fault(
                "delivery_capacity",
                "Pending delivery could not be closed; retain its original identity and reconcile",
            ));
        }
        self.db
            .resource_batch(&[(self.marker(), self.marker_bytes()?), (self.key(id), bytes)])
            .map_err(unavailable)?;
        Ok(receipt)
    }
    /// The session, its monotonic revision, optional newly bound task and input
    /// receipt share one synchronous WAL commit. No runner has started yet.
    pub fn accept(&self, acceptance: Acceptance<'_>) -> Result<DeliveryReceipt> {
        let _guard = self.db.resource_gate.lock().map_err(unavailable)?;
        self.check_schema()?;
        if let Some(receipt) = self.lookup(acceptance.identity.id, acceptance.input_digest)? {
            if receipt.live.is_some() {
                return Err(fault(
                    "request_conflict",
                    "Input identity already belongs to a live queue",
                ));
            }
            return Ok(receipt);
        }
        let rows = self
            .db
            .resource_scan(&self.records_prefix(), &[], MAX_RECEIPTS + 1)
            .map_err(unavailable)?;
        let current =
            self.db.chat_revision(acceptance.context.session_id.as_bytes()).map_err(unavailable)?;
        if current != acceptance.identity.expected_revision {
            return Err(fault(
                "session_conflict",
                "Conversation changed. Review its current revision before sending.",
            ));
        }
        let receipt = DeliveryReceipt {
            live: None,
            schema_version: 1,
            id: acceptance.identity.id,
            input_digest: acceptance.input_digest.into(),
            session: acceptance.context.session_id,
            turn: acceptance.turn,
            checkpoint: acceptance.checkpoint.into(),
            accepted_revision: current
                .checked_add(1)
                .ok_or_else(|| fault("revision_exhausted", "Session revision exhausted"))?,
            phase: DeliveryPhase::Accepted,
            outcome: None,
            accepted_ms: acceptance.now,
            updated_ms: acceptance.now,
        };
        let bytes = serde_json::to_vec(&receipt).map_err(unavailable)?;
        if bytes.len() > MAX_RECEIPT_BYTES
            || rows.len() >= MAX_RECEIPTS
            || rows
                .iter()
                .map(|(_, raw)| raw.len())
                .sum::<usize>()
                .saturating_add(self.live_body_bytes()?)
                .saturating_add(bytes.len())
                > MAX_LEDGER_BYTES
        {
            return Err(fault(
                "delivery_capacity",
                "Delivery ledger capacity reached. Existing receipts were retained; this input was not accepted.",
            ));
        }
        let context = serde_json::to_vec(acceptance.context).map_err(unavailable)?;
        let mut values = vec![(self.marker(), self.marker_bytes()?), (self.key(receipt.id), bytes)];
        if let Some(handle) = acceptance.task {
            values.extend(
                handle
                    .store
                    .accepted_input_values(
                        handle.id,
                        receipt.session,
                        current,
                        receipt.accepted_revision,
                    )
                    .map_err(unavailable)?,
            );
        }
        self.db
            .continuation_batch(&values, Some((receipt.session.as_bytes(), &context, current)))
            .map_err(unavailable)?;
        Ok(receipt)
    }
    /// Started is durable before entering the existing runner. A crash in this
    /// phase has uncertain effects and never authorizes a replacement run.
    pub fn transition(
        &self,
        id: Uuid,
        phase: DeliveryPhase,
        outcome: Option<&str>,
        now: u64,
    ) -> Result<DeliveryReceipt> {
        let _guard = self.db.resource_gate.lock().map_err(unavailable)?;
        let mut receipt =
            self.get(id)?.ok_or_else(|| fault("not_found", "Delivery receipt missing"))?;
        if receipt.live.is_some() {
            return Err(fault("invalid_transition", "Live input uses its owned turn lifecycle"));
        }
        if receipt.phase == DeliveryPhase::Finished
            || receipt.phase == DeliveryPhase::NeedsAttention
            || receipt.phase == DeliveryPhase::Retired
        {
            return Ok(receipt);
        }
        if receipt.phase == DeliveryPhase::Started && phase == DeliveryPhase::Accepted {
            return Err(fault(
                "invalid_transition",
                "An input cannot return to unstarted acceptance",
            ));
        }
        if outcome.is_some_and(|value| value.len() > 256) {
            return Err(fault("invalid_outcome", "Delivery outcome must be bounded metadata"));
        }
        receipt.phase = phase;
        receipt.outcome = outcome.map(str::to_owned);
        receipt.updated_ms = now;
        self.check_replacement(id, serde_json::to_vec(&receipt).map_err(unavailable)?.len())?;
        self.db
            .resource_batch(&[(self.key(id), serde_json::to_vec(&receipt).map_err(unavailable)?)])
            .map_err(unavailable)?;
        Ok(receipt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::session::load_thread;
    use metteur_shared::llm::{ContextManager, Message, Role};
    fn setup() -> (std::path::PathBuf, Db, DeliveryStore, ChatSessionRecord) {
        let root = std::env::temp_dir().join(format!("dw-delivery-{}", Uuid::new_v4()));
        let db = Db::open(&root).unwrap();
        let store = DeliveryStore::new(db.clone(), "canonical-workspace", "local").unwrap();
        let mut context = ContextManager::default();
        context.push_message(Message::text(Role::User, "original\r\n中文"));
        (root, db, store, ChatSessionRecord::new(1, context))
    }
    fn input<'a>(identity: &'a DeliveryIdentity, context: &'a ChatSessionRecord) -> Acceptance<'a> {
        Acceptance {
            identity,
            input_digest: "input-digest",
            context,
            turn: Uuid::new_v4(),
            checkpoint: "checkpoint",
            task: None,
            now: 1,
        }
    }
    #[test]
    fn acceptance_and_user_context_survive_reopen_without_a_runner() {
        let (root, db, store, context) = setup();
        let identity = DeliveryIdentity {
            id: Uuid::new_v4(),
            expected_revision: 0,
            new_session: None,
        };
        let accepted = store.accept(input(&identity, &context)).unwrap();
        assert_eq!(db.chat_revision(context.session_id.as_bytes()).unwrap(), 1);
        assert_eq!(accepted.phase, DeliveryPhase::Accepted);
        drop(store);
        drop(db);
        let reopened = Db::open(&root).unwrap();
        let store = DeliveryStore::new(reopened.clone(), "canonical-workspace", "local").unwrap();
        let record = load_thread(&reopened, &context.session_id).unwrap().unwrap();
        assert_eq!(record.context.messages.last().unwrap().text_content(), "original\r\n中文");
        assert_eq!(store.get(identity.id).unwrap().unwrap().turn, accepted.turn);
        assert_eq!(store.get(identity.id).unwrap().unwrap().phase, DeliveryPhase::Accepted);
    }
    #[test]
    fn exact_replay_cannot_advance_revision_or_reassign_turn_and_different_input_is_rejected() {
        let (_, db, store, context) = setup();
        let identity = DeliveryIdentity {
            id: Uuid::new_v4(),
            expected_revision: 0,
            new_session: None,
        };
        let accepted = store.accept(input(&identity, &context)).unwrap();
        let replay = store.accept(input(&identity, &context)).unwrap();
        assert_eq!(accepted.turn, replay.turn);
        assert_eq!(db.chat_revision(context.session_id.as_bytes()).unwrap(), 1);
        assert_eq!(store.lookup(identity.id, "other-input").unwrap_err().code, "request_conflict");
        let next = DeliveryIdentity {
            id: Uuid::new_v4(),
            expected_revision: 0,
            new_session: None,
        };
        assert_eq!(store.accept(input(&next, &context)).unwrap_err().code, "session_conflict");
        assert!(store.get(next.id).unwrap().is_none());
    }
    #[test]
    fn owner_and_workspace_queries_do_not_expose_another_receipt() {
        let (_, db, store, context) = setup();
        let identity = DeliveryIdentity {
            id: Uuid::new_v4(),
            expected_revision: 0,
            new_session: None,
        };
        store.accept(input(&identity, &context)).unwrap();
        for (workspace, subject) in
            [("canonical-workspace", "another"), ("other-workspace", "local")]
        {
            let other = DeliveryStore::new(db.clone(), workspace, subject).unwrap();
            assert!(other.get(identity.id).unwrap().is_none());
        }
    }
    #[test]
    fn capacity_rejects_new_input_without_changing_its_session_or_eviction() {
        let (_, db, store, context) = setup();
        let rows: Vec<_> = (0..MAX_RECEIPTS)
            .map(|_| (store.key(Uuid::new_v4()), b"unrecognized retained receipt".to_vec()))
            .collect();
        db.resource_batch(&rows).unwrap();
        let identity = DeliveryIdentity {
            id: Uuid::new_v4(),
            expected_revision: 0,
            new_session: None,
        };
        assert_eq!(store.accept(input(&identity, &context)).unwrap_err().code, "delivery_capacity");
        assert_eq!(db.chat_revision(context.session_id.as_bytes()).unwrap(), 0);
        assert!(load_thread(&db, &context.session_id).unwrap().is_none());
        assert_eq!(
            db.resource_scan(&store.records_prefix(), &[], MAX_RECEIPTS + 1).unwrap().len(),
            MAX_RECEIPTS
        );
    }
    #[test]
    fn started_or_finished_receipt_cannot_be_reset_to_replay_the_input() {
        let (_, _, store, context) = setup();
        let identity = DeliveryIdentity {
            id: Uuid::new_v4(),
            expected_revision: 0,
            new_session: None,
        };
        store.accept(input(&identity, &context)).unwrap();
        store.transition(identity.id, DeliveryPhase::Started, None, 2).unwrap();
        assert_eq!(
            store.transition(identity.id, DeliveryPhase::Accepted, None, 3).unwrap_err().code,
            "invalid_transition"
        );
        store.transition(identity.id, DeliveryPhase::Finished, Some("error"), 4).unwrap();
        let replay = store.accept(input(&identity, &context)).unwrap();
        assert_eq!(replay.phase, DeliveryPhase::Finished);
        assert_eq!(replay.outcome.as_deref(), Some("error"));
    }
    #[test]
    fn failed_wal_commit_neither_accepts_input_nor_advances_the_session() {
        let (_, db, store, context) = setup();
        let identity = DeliveryIdentity {
            id: Uuid::new_v4(),
            expected_revision: 0,
            new_session: None,
        };
        db.fail_delivery_write.store(true, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            store.accept(input(&identity, &context)).unwrap_err().code,
            "storage_unavailable"
        );
        assert!(store.get(identity.id).unwrap().is_none());
        assert!(load_thread(&db, &context.session_id).unwrap().is_none());
        assert_eq!(db.chat_revision(context.session_id.as_bytes()).unwrap(), 0);
        assert_eq!(store.accept(input(&identity, &context)).unwrap().accepted_revision, 1);
    }
    #[test]
    fn missing_receipt_is_not_proof_until_its_identity_is_durably_retired() {
        let (_, db, store, context) = setup();
        let identity = DeliveryIdentity {
            id: Uuid::new_v4(),
            expected_revision: 0,
            new_session: None,
        };
        assert!(store.get(identity.id).unwrap().is_none());
        assert_eq!(store.retire(identity.id, 2).unwrap().phase, DeliveryPhase::Retired);
        assert_eq!(store.accept(input(&identity, &context)).unwrap_err().code, "delivery_retired");
        assert!(load_thread(&db, &context.session_id).unwrap().is_none());
        assert_eq!(db.chat_revision(context.session_id.as_bytes()).unwrap(), 0);
    }
    #[test]
    fn retirement_after_acceptance_returns_the_original_turn_without_cancelling_it() {
        let (_, _, store, context) = setup();
        let identity = DeliveryIdentity {
            id: Uuid::new_v4(),
            expected_revision: 0,
            new_session: None,
        };
        let original = store.accept(input(&identity, &context)).unwrap();
        let closed = store.retire(identity.id, 2).unwrap();
        assert_eq!(closed.turn, original.turn);
        assert_eq!(closed.phase, DeliveryPhase::Accepted);
    }
}
