//! Decision receipts fence the existing broker; they never replay a grant.
use super::{
    persistence::{Db, cf},
    resources::{Result, digest, fault},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MAX_RECEIPTS: usize = 8192;
pub const MAX_BYTES: usize = 16 * 1024 * 1024;
const PREFIX: &[u8] = b"dw/approval-decisions/";
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub kind: String,
    pub session: Option<Uuid>,
    pub turn: Option<Uuid>,
    pub run: Option<Uuid>,
}
impl Target {
    pub fn valid(&self) -> bool {
        match self.kind.as_str() {
            "chat" => {
                self.session.is_some_and(|id| !id.is_nil())
                    && self.turn.is_some_and(|id| !id.is_nil())
                    && self.run.is_none()
            }
            "blueprint" => {
                self.run.is_some_and(|id| !id.is_nil())
                    && self.session.is_none()
                    && self.turn.is_none()
            }
            _ => false,
        }
    }
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Claimed,
    Accepted,
    Unknown,
    Retired,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub schema_version: u32,
    pub id: Uuid,
    pub owner: String,
    pub target: Target,
    pub content_digest: String,
    pub decision: Option<String>,
    pub phase: Phase,
}
pub struct ApprovalDelivery {
    db: Db,
    owner: String,
}
impl ApprovalDelivery {
    pub fn new(db: Db, owner: &str) -> Result<Self> {
        if owner.is_empty() || owner.len() > 512 {
            return Err(fault("invalid_owner", "Approval requires an authenticated owner"));
        }
        Ok(Self {
            db,
            owner: owner.into(),
        })
    }
    fn key(id: Uuid) -> Vec<u8> {
        [PREFIX, id.to_string().as_bytes()].concat()
    }
    fn marker() -> Vec<u8> {
        [PREFIX, b"schema"].concat()
    }
    fn check(&self) -> Result<()> {
        if self
            .db
            .get(cf::EXECUTION_STATE, &Self::marker())
            .map_err(storage)?
            .is_some_and(|value| value != b"1")
        {
            return Err(fault(
                "unsupported_schema",
                "Approval receipts require another application version",
            ));
        }
        Ok(())
    }
    pub fn read(&self, id: Uuid) -> Result<Option<Receipt>> {
        self.check()?;
        let Some(raw) = self.db.get(cf::EXECUTION_STATE, &Self::key(id)).map_err(storage)? else {
            return Ok(None);
        };
        if raw.len() > 4096 {
            return Err(fault("receipt_limit", "Approval receipt exceeds the supported limit"));
        }
        let record: Receipt = serde_json::from_slice(&raw).map_err(|_| {
            fault("corrupt_receipt", "Approval receipt is unavailable; it will not be replayed")
        })?;
        if record.schema_version != 1 || record.id != id || !record.target.valid() {
            return Err(fault(
                "unsupported_schema",
                "Approval receipt is unsupported; retain it for review",
            ));
        }
        if record.owner != self.owner {
            return Err(fault("permission_denied", "Approval receipt belongs to another subject"));
        }
        Ok(Some(record))
    }
    fn write(&self, record: &Receipt) -> Result<()> {
        self.check()?;
        let raw = serde_json::to_vec(record).map_err(storage)?;
        if raw.len() > 4096 {
            return Err(fault("receipt_limit", "Approval decision exceeds the receipt limit"));
        }
        let rows = self.db.resource_scan(PREFIX, &[], MAX_RECEIPTS + 2).map_err(storage)?;
        let key = Self::key(record.id);
        let existing = rows.iter().any(|(candidate, _)| *candidate == key);
        let count = rows.iter().filter(|(candidate, _)| *candidate != Self::marker()).count();
        let bytes = rows
            .iter()
            .filter(|(candidate, _)| *candidate != key)
            .map(|(_, body)| body.len())
            .sum::<usize>()
            + raw.len();
        if (!existing && count >= MAX_RECEIPTS)
            || rows.len() > MAX_RECEIPTS + 1
            || bytes > MAX_BYTES
        {
            return Err(fault(
                "receipt_capacity",
                "Approval receipt capacity reached; existing decisions were retained",
            ));
        }
        self.db
            .approval_receipt_batch(&[(Self::marker(), b"1".to_vec()), (key, raw)])
            .map_err(storage)
    }
    fn validate(id: Uuid, target: &Target, content_digest: &str) -> Result<()> {
        if id.is_nil()
            || !target.valid()
            || content_digest.len() != 64
            || !content_digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(fault(
                "invalid_request",
                "An exact approval identity, target and content digest are required",
            ));
        }
        Ok(())
    }
    /// The first caller durably claims before touching the broker/grants.
    /// Claimed/unknown receipts can only be inspected, never re-executed.
    pub fn claim(
        &self,
        id: Uuid,
        target: &Target,
        content_digest: &str,
        decision: &str,
    ) -> Result<(Receipt, bool)> {
        Self::validate(id, target, content_digest)?;
        if crate::sandbox::approval::parse_decision(decision).is_none() {
            return Err(fault("invalid_request", "Unknown approval decision"));
        }
        let _gate = self.db.resource_gate.lock().map_err(storage)?;
        if let Some(record) = self.read(id)? {
            if record.phase == Phase::Retired {
                return Err(fault("retired", "Approval decision was closed before receipt"));
            }
            if record.target != *target
                || record.content_digest != content_digest
                || record.decision.as_deref() != Some(decision)
            {
                return Err(fault(
                    "identity_conflict",
                    "This approval identity already has a different target, content or decision",
                ));
            }
            return Ok((record, false));
        }
        let record = Receipt {
            schema_version: 1,
            id,
            owner: self.owner.clone(),
            target: target.clone(),
            content_digest: content_digest.into(),
            decision: Some(decision.into()),
            phase: Phase::Claimed,
        };
        self.write(&record)?;
        Ok((record, true))
    }
    pub fn finish(&self, id: Uuid, accepted: bool) -> Result<Receipt> {
        let _gate = self.db.resource_gate.lock().map_err(storage)?;
        let mut record =
            self.read(id)?.ok_or_else(|| fault("not_found", "Approval receipt not found"))?;
        if record.phase != Phase::Claimed {
            return Ok(record);
        }
        record.phase = if accepted {
            Phase::Accepted
        } else {
            Phase::Unknown
        };
        self.write(&record)?;
        Ok(record)
    }
    pub fn read_or_retire(
        &self,
        id: Uuid,
        target: &Target,
        content_digest: &str,
    ) -> Result<Receipt> {
        Self::validate(id, target, content_digest)?;
        let _gate = self.db.resource_gate.lock().map_err(storage)?;
        if let Some(record) = self.read(id)? {
            if record.target != *target || record.content_digest != content_digest {
                return Err(fault(
                    "identity_conflict",
                    "Approval reconciliation target or content differs",
                ));
            }
            return Ok(record);
        }
        let record = Receipt {
            schema_version: 1,
            id,
            owner: self.owner.clone(),
            target: target.clone(),
            content_digest: content_digest.into(),
            decision: None,
            phase: Phase::Retired,
        };
        self.write(&record)?;
        Ok(record)
    }
    pub fn content_digest(raw: &str) -> String {
        digest(raw.as_bytes())
    }
}
fn storage(_: impl std::fmt::Display) -> metteur_shared::resources::ResourceError {
    fault(
        "storage_unavailable",
        "Approval receipt could not be confirmed; retain its original identity",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn setup() -> (std::path::PathBuf, Db, ApprovalDelivery, Target, String) {
        let root = std::env::temp_dir().join(format!("dw-approval-ledger-{}", Uuid::new_v4()));
        let db = Db::open(&root).unwrap();
        let store = ApprovalDelivery::new(db.clone(), "local").unwrap();
        let target = Target {
            kind: "chat".into(),
            session: Some(Uuid::new_v4()),
            turn: Some(Uuid::new_v4()),
            run: None,
        };
        (root, db, store, target, ApprovalDelivery::content_digest("immutable command\r\n中文"))
    }
    #[test]
    fn accepted_decision_survives_reopen_and_never_replays_or_escalates() {
        let (root, db, store, target, digest) = setup();
        let id = Uuid::new_v4();
        assert!(store.claim(id, &target, &digest, "AllowOnce").unwrap().1);
        assert!(!store.claim(id, &target, &digest, "AllowOnce").unwrap().1);
        assert!(store.claim(id, &target, &digest, "AllowGlobal").is_err());
        let mut other = target.clone();
        other.turn = Some(Uuid::new_v4());
        assert!(store.claim(id, &other, &digest, "AllowOnce").is_err());
        assert!(
            store
                .claim(id, &target, &ApprovalDelivery::content_digest("different"), "AllowOnce")
                .is_err()
        );
        assert!(ApprovalDelivery::new(db.clone(), "other").unwrap().read(id).is_err());
        assert_eq!(store.finish(id, true).unwrap().phase, Phase::Accepted);
        drop(store);
        drop(db);
        let reopened = ApprovalDelivery::new(Db::open(&root).unwrap(), "local").unwrap();
        assert_eq!(reopened.read(id).unwrap().unwrap().phase, Phase::Accepted);
        assert!(!reopened.claim(id, &target, &digest, "AllowOnce").unwrap().1);
    }
    #[test]
    fn retirement_and_unknown_outcomes_fence_all_late_decisions() {
        let (_, _, store, target, digest) = setup();
        let id = Uuid::new_v4();
        assert_eq!(store.read_or_retire(id, &target, &digest).unwrap().phase, Phase::Retired);
        assert!(store.claim(id, &target, &digest, "DenyOnce").is_err());
        let claimed = Uuid::new_v4();
        store.claim(claimed, &target, &digest, "AllowWorkspace").unwrap();
        assert_eq!(store.read_or_retire(claimed, &target, &digest).unwrap().phase, Phase::Claimed);
        assert_eq!(store.finish(claimed, false).unwrap().phase, Phase::Unknown);
        assert!(!store.claim(claimed, &target, &digest, "AllowWorkspace").unwrap().1);
    }
    #[test]
    fn failed_wal_never_claims_and_future_or_corrupt_receipts_are_retained() {
        let (_, db, store, target, digest) = setup();
        let id = Uuid::new_v4();
        db.fail_delivery_write.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(store.claim(id, &target, &digest, "AllowOnce").is_err());
        assert!(store.read(id).unwrap().is_none());
        store.claim(id, &target, &digest, "AllowOnce").unwrap();
        db.fail_delivery_write.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(store.finish(id, true).is_err());
        assert_eq!(store.read(id).unwrap().unwrap().phase, Phase::Claimed);
        let raw = db.get(cf::EXECUTION_STATE, &ApprovalDelivery::key(id)).unwrap().unwrap();
        db.put(cf::EXECUTION_STATE, &ApprovalDelivery::marker(), b"99").unwrap();
        assert!(store.read(id).is_err());
        assert_eq!(db.get(cf::EXECUTION_STATE, &ApprovalDelivery::key(id)).unwrap().unwrap(), raw);
        db.put(cf::EXECUTION_STATE, &ApprovalDelivery::marker(), b"1").unwrap();
        db.put(cf::EXECUTION_STATE, &ApprovalDelivery::key(id), b"{corrupt").unwrap();
        assert!(store.read(id).is_err());
        assert!(store.claim(id, &target, &digest, "AllowOnce").is_err());
    }
    #[test]
    fn claim_and_retirement_have_one_atomic_winner() {
        let (_, db, store, target, digest) = setup();
        let id = Uuid::new_v4();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let gate = barrier.clone();
        let other_target = target.clone();
        let other_digest = digest.clone();
        let other = ApprovalDelivery::new(db, "local").unwrap();
        let contender = std::thread::spawn(move || {
            gate.wait();
            other.claim(id, &other_target, &other_digest, "AllowOnce")
        });
        barrier.wait();
        let retired = store.read_or_retire(id, &target, &digest).unwrap();
        let claimed = contender.join().unwrap();
        match retired.phase {
            Phase::Retired => assert!(claimed.is_err()),
            Phase::Claimed => assert!(claimed.unwrap().1),
            _ => panic!("unexpected phase"),
        }
    }
    #[test]
    fn capacity_rejection_does_not_evict_old_decisions() {
        let (_, db, store, target, digest) = setup();
        let mut records = Vec::new();
        for _ in 0..MAX_RECEIPTS {
            let record = Receipt {
                schema_version: 1,
                id: Uuid::new_v4(),
                owner: "local".into(),
                target: target.clone(),
                content_digest: digest.clone(),
                decision: None,
                phase: Phase::Retired,
            };
            records.push((ApprovalDelivery::key(record.id), serde_json::to_vec(&record).unwrap()));
        }
        let first = records[0].clone();
        db.approval_receipt_batch(&records).unwrap();
        assert_eq!(
            store.claim(Uuid::new_v4(), &target, &digest, "AllowOnce").unwrap_err().code,
            "receipt_capacity"
        );
        assert_eq!(db.get(cf::EXECUTION_STATE, &first.0).unwrap().unwrap(), first.1);
    }
}
