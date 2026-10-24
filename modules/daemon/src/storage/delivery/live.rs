//! Durable bodies for queued input, outside execution rewind and runner state.
use super::*;

pub const MAX_LIVE_BODY: usize = 1024 * 1024;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveMetadata {
    pub mode: String,
    pub body_digest: String,
}

pub struct LiveAcceptance<'a> {
    pub id: Uuid,
    pub input_digest: &'a str,
    pub session: Uuid,
    pub turn: Uuid,
    pub mode: &'a str,
    pub body: &'a str,
    pub now: u64,
}
impl DeliveryStore {
    fn body_prefix(&self) -> Vec<u8> {
        format!("{}bodies/", self.prefix).into_bytes()
    }
    fn body_key(&self, id: Uuid) -> Vec<u8> {
        format!("{}bodies/{id}", self.prefix).into_bytes()
    }
    pub(super) fn marker_bytes(&self) -> Result<Vec<u8>> {
        self.check_schema()?;
        Ok(
            if self
                .db
                .get(cf::EXECUTION_STATE, &self.marker())
                .map_err(unavailable)?
                .is_some_and(|raw| raw == b"2")
            {
                b"2".to_vec()
            } else {
                b"1".to_vec()
            },
        )
    }
    pub(super) fn live_body_bytes(&self) -> Result<usize> {
        let rows = self
            .db
            .resource_scan(&self.body_prefix(), &[], MAX_RECEIPTS + 1)
            .map_err(unavailable)?;
        if rows.len() > MAX_RECEIPTS {
            return Err(fault(
                "delivery_capacity",
                "Input body count exceeds its bounded namespace",
            ));
        }
        Ok(rows.iter().fold(0usize, |sum, (_, raw)| sum.saturating_add(raw.len())))
    }
    pub(super) fn check_replacement(&self, id: Uuid, new_len: usize) -> Result<()> {
        let rows = self
            .db
            .resource_scan(&self.records_prefix(), &[], MAX_RECEIPTS + 1)
            .map_err(unavailable)?;
        let key = self.key(id);
        let old = rows.iter().find(|(entry, _)| entry == &key).map_or(0, |(_, raw)| raw.len());
        let total = rows
            .iter()
            .fold(0usize, |sum, (_, raw)| sum.saturating_add(raw.len()))
            .saturating_add(self.live_body_bytes()?)
            .saturating_sub(old)
            .saturating_add(new_len);
        if rows.len() > MAX_RECEIPTS || new_len > MAX_RECEIPT_BYTES || total > MAX_LEDGER_BYTES {
            return Err(fault(
                "delivery_capacity",
                "Receipt metadata is unconfirmed; existing input bodies were retained",
            ));
        }
        Ok(())
    }
    pub fn live_body(&self, id: Uuid) -> Result<Option<String>> {
        let Some(receipt) = self.get(id)? else {
            return Ok(None);
        };
        let Some(live) = receipt.live else {
            return Ok(None);
        };
        let raw =
            self.db.get(cf::EXECUTION_STATE, &self.body_key(id)).map_err(unavailable)?.ok_or_else(
                || fault("corrupt_input", "Received live input body is unavailable; do not replay"),
            )?;
        if raw.len() > MAX_LIVE_BODY || digest(&raw) != live.body_digest {
            return Err(fault("corrupt_input", "Received input requires review; do not replay"));
        }
        String::from_utf8(raw)
            .map(Some)
            .map_err(|_| fault("corrupt_input", "Received input is not valid UTF-8; do not replay"))
    }
    /// Called while the existing bus fences acceptance against turn closure.
    pub fn accept_live(&self, input: LiveAcceptance<'_>) -> Result<(DeliveryReceipt, bool)> {
        let _guard = self.db.resource_gate.lock().map_err(unavailable)?;
        self.check_schema()?;
        if let Some(receipt) = self.lookup(input.id, input.input_digest)? {
            if receipt.live.is_none() {
                return Err(fault(
                    "request_conflict",
                    "Input identity already belongs to a manual turn",
                ));
            }
            return Ok((receipt, false));
        }
        if input.id.is_nil()
            || input.session.is_nil()
            || input.turn.is_nil()
            || !["queue", "steer"].contains(&input.mode)
            || input.body.trim().is_empty()
            || input.body.len() > MAX_LIVE_BODY
        {
            return Err(fault(
                "invalid_input",
                "Live input requires a bounded body and concrete identity",
            ));
        }
        let rows = self
            .db
            .resource_scan(&self.records_prefix(), &[], MAX_RECEIPTS + 1)
            .map_err(unavailable)?;
        let receipt = DeliveryReceipt {
            schema_version: 1,
            id: input.id,
            input_digest: input.input_digest.into(),
            session: input.session,
            turn: input.turn,
            checkpoint: String::new(),
            accepted_revision: self
                .db
                .chat_revision(input.session.as_bytes())
                .map_err(unavailable)?,
            phase: DeliveryPhase::LiveQueued,
            outcome: None,
            accepted_ms: input.now,
            updated_ms: input.now,
            live: Some(LiveMetadata {
                mode: input.mode.into(),
                body_digest: digest(input.body.as_bytes()),
            }),
        };
        let bytes = serde_json::to_vec(&receipt).map_err(unavailable)?;
        let total = rows
            .iter()
            .fold(0usize, |sum, (_, raw)| sum.saturating_add(raw.len()))
            .saturating_add(self.live_body_bytes()?)
            .saturating_add(bytes.len())
            .saturating_add(input.body.len());
        if bytes.len() > MAX_RECEIPT_BYTES || rows.len() >= MAX_RECEIPTS || total > MAX_LEDGER_BYTES
        {
            return Err(fault(
                "delivery_capacity",
                "Live input could not be received; existing bodies were retained",
            ));
        }
        // Marker 2 makes older hosts refuse ledger writes instead of ignoring
        // the body quota or interpreting a queued input as a manual turn.
        self.db
            .continuation_batch(
                &[
                    (self.marker(), b"2".to_vec()),
                    (self.key(input.id), bytes),
                    (self.body_key(input.id), input.body.as_bytes().to_vec()),
                ],
                None,
            )
            .map_err(unavailable)?;
        Ok((receipt, true))
    }
    pub fn transition_live(
        &self,
        id: Uuid,
        phase: DeliveryPhase,
        now: u64,
    ) -> Result<DeliveryReceipt> {
        let _guard = self.db.resource_gate.lock().map_err(unavailable)?;
        let mut receipt =
            self.get(id)?.ok_or_else(|| fault("not_found", "Live input receipt is missing"))?;
        if receipt.live.is_none() {
            return Err(fault("invalid_transition", "Manual input is not a queued input"));
        }
        if receipt.phase == phase {
            return Ok(receipt);
        }
        if !matches!(
            (&receipt.phase, &phase),
            (DeliveryPhase::LiveQueued, DeliveryPhase::LiveClaimed | DeliveryPhase::LiveUnconsumed)
                | (
                    DeliveryPhase::LiveClaimed,
                    DeliveryPhase::LiveApplied | DeliveryPhase::LiveUnknown
                )
        ) {
            return Err(fault(
                "invalid_transition",
                "Live input cannot be replayed or moved to another turn",
            ));
        }
        receipt.phase = phase;
        receipt.updated_ms = now;
        let bytes = serde_json::to_vec(&receipt).map_err(unavailable)?;
        self.check_replacement(id, bytes.len())?;
        self.db.continuation_batch(&[(self.key(id), bytes)], None).map_err(unavailable)?;
        Ok(receipt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn setup() -> (std::path::PathBuf, Db, DeliveryStore) {
        let root = std::env::temp_dir().join(format!("live-input-ledger-{}", Uuid::new_v4()));
        let db = Db::open(&root).unwrap();
        let store = DeliveryStore::new(db.clone(), "workspace", "owner").unwrap();
        (root, db, store)
    }
    fn input(id: Uuid) -> LiveAcceptance<'static> {
        LiveAcceptance {
            id,
            input_digest: "bound-input",
            session: Uuid::from_u128(7),
            turn: Uuid::from_u128(8),
            mode: "queue",
            body: "  raw\r\n中文 input  ",
            now: 1,
        }
    }
    #[test]
    fn body_and_receipt_survive_reopen_and_exact_replay_is_not_new_acceptance() {
        let (root, db, store) = setup();
        let id = Uuid::new_v4();
        assert!(store.accept_live(input(id)).unwrap().1);
        assert!(!store.accept_live(input(id)).unwrap().1);
        assert_eq!(store.live_body(id).unwrap().as_deref(), Some(input(id).body));
        assert_eq!(db.get(cf::EXECUTION_STATE, &store.marker()).unwrap().unwrap(), b"2");
        drop(store);
        drop(db);
        let db = Db::open(&root).unwrap();
        let store = DeliveryStore::new(db, "workspace", "owner").unwrap();
        assert_eq!(store.get(id).unwrap().unwrap().phase, DeliveryPhase::LiveQueued);
        assert_eq!(store.live_body(id).unwrap().as_deref(), Some(input(id).body));
    }
    #[test]
    fn wal_failure_admits_neither_body_nor_id_and_retirement_fences_a_late_retry() {
        let (_, db, store) = setup();
        let id = Uuid::new_v4();
        db.fail_delivery_write.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(store.accept_live(input(id)).is_err());
        assert!(store.get(id).unwrap().is_none());
        assert!(db.get(cf::EXECUTION_STATE, &store.body_key(id)).unwrap().is_none());
        store.retire(id, 2).unwrap();
        assert_eq!(store.accept_live(input(id)).unwrap_err().code, "delivery_retired");
    }
    #[test]
    fn terminal_live_dispositions_cannot_replay_or_become_manual_turns() {
        let (_, _, store) = setup();
        let unused = Uuid::new_v4();
        store.accept_live(input(unused)).unwrap();
        store.transition_live(unused, DeliveryPhase::LiveUnconsumed, 2).unwrap();
        assert!(store.transition_live(unused, DeliveryPhase::LiveClaimed, 3).is_err());
        let used = Uuid::new_v4();
        store.accept_live(input(used)).unwrap();
        store.transition_live(used, DeliveryPhase::LiveClaimed, 2).unwrap();
        store.transition_live(used, DeliveryPhase::LiveApplied, 3).unwrap();
        assert!(store.transition_live(used, DeliveryPhase::LiveQueued, 4).is_err());
        assert!(store.transition(used, DeliveryPhase::Started, None, 4).is_err());
        assert_eq!(store.live_body(unused).unwrap().as_deref(), Some(input(unused).body));
    }
    #[test]
    fn marker_is_never_downgraded_and_unknown_or_corrupt_data_fails_closed() {
        let (_, db, store) = setup();
        let id = Uuid::new_v4();
        store.accept_live(input(id)).unwrap();
        store.retire(Uuid::new_v4(), 2).unwrap();
        assert_eq!(db.get(cf::EXECUTION_STATE, &store.marker()).unwrap().unwrap(), b"2");
        db.put_durable(cf::EXECUTION_STATE, &store.body_key(id), b"corrupt").unwrap();
        assert_eq!(store.live_body(id).unwrap_err().code, "corrupt_input");
        db.put_durable(cf::EXECUTION_STATE, &store.marker(), b"3").unwrap();
        assert_eq!(store.get(id).unwrap_err().code, "unsupported_schema");
    }
    #[test]
    fn changed_content_is_rejected_and_another_owner_cannot_read_the_body() {
        let (_, db, store) = setup();
        let id = Uuid::new_v4();
        store.accept_live(input(id)).unwrap();
        let mut changed = input(id);
        changed.input_digest = "different";
        assert_eq!(store.accept_live(changed).unwrap_err().code, "request_conflict");
        assert!(
            DeliveryStore::new(db, "workspace", "another")
                .unwrap()
                .live_body(id)
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn body_quota_also_fences_manual_acceptance_without_eviction() {
        let (_, db, store) = setup();
        db.put_durable(
            cf::EXECUTION_STATE,
            &store.body_key(Uuid::new_v4()),
            &vec![b'x'; MAX_LEDGER_BYTES - 100],
        )
        .unwrap();
        let context = crate::chat::session::ChatSessionRecord::new(1, Default::default());
        let id = Uuid::new_v4();
        assert_eq!(
            store
                .accept(Acceptance {
                    identity: &DeliveryIdentity {
                        id,
                        expected_revision: 0,
                        new_session: None
                    },
                    input_digest: "manual",
                    context: &context,
                    turn: Uuid::new_v4(),
                    checkpoint: "checkpoint",
                    task: None,
                    now: 2
                })
                .unwrap_err()
                .code,
            "delivery_capacity"
        );
        assert!(store.get(id).unwrap().is_none());
        assert_eq!(store.live_body_bytes().unwrap(), MAX_LEDGER_BYTES - 100);
    }
}
