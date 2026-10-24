//! Bounded receipts for existing direct controls and blueprint temporary input.
//! These records never execute work, replay input, or participate in rewind.
use super::{
    approval_delivery::Target,
    persistence::{Db, cf},
    resources::{Result, digest, fault},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const PREFIX: &[u8] = b"dw/workflow-receipts/";
const LIMIT: usize = 8192;
const BYTES: usize = 16 * 1024 * 1024;
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Prepared,
    Claimed,
    Accepted,
    Queued,
    Applied,
    Unconsumed,
    Unknown,
    Retired,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn setup() -> (std::path::PathBuf, Db, Store, Target) {
        let root = std::env::temp_dir().join(format!("dw-workflow-receipts-{}", Uuid::new_v4()));
        let db = Db::open(&root).unwrap();
        let store = Store::new(db.clone(), "local").unwrap();
        (
            root,
            db,
            store,
            Target {
                kind: "blueprint".into(),
                run: Some(Uuid::new_v4()),
                session: None,
                turn: None,
            },
        )
    }
    fn identity(revision: u64) -> Identity {
        Identity {
            id: Uuid::new_v4(),
            revision,
        }
    }
    #[test]
    fn controls_are_ordered_and_duplicates_never_reverse_a_later_decision() {
        let (root, db, store, target) = setup();
        let paused = std::cell::Cell::new(false);
        let first = identity(0);
        store.prepare(&first, &target, "pause", None).unwrap();
        store.control(&first, &target, "pause", || paused.set(true)).unwrap();
        store.control(&identity(1), &target, "resume", || paused.set(false)).unwrap();
        store.control(&first, &target, "pause", || panic!("duplicate applied")).unwrap();
        assert!(!paused.get());
        assert!(store.control(&identity(0), &target, "pause", || panic!("stale applied")).is_err());
        assert!(Store::new(db.clone(), "other").unwrap().read(first.id).is_err());
        assert_eq!(store.list(None).unwrap().0.len(), 2);
        drop(store);
        drop(db);
        let reopened = Store::new(Db::open(&root).unwrap(), "local").unwrap();
        assert_eq!(reopened.revision(&target).unwrap(), 2);
        assert_eq!(reopened.read(first.id).unwrap().unwrap().phase, Phase::Accepted);
    }
    #[test]
    fn fences_win_against_late_input_and_consumption_does_not_replay_after_restart() {
        let (_, _, store, target) = setup();
        let retired = identity(0);
        store.prepare(&retired, &target, "temporary_input", Some("raw\r\n中文")).unwrap();
        assert_eq!(
            store.fence(retired.id, &target, "temporary_input", Some("raw\r\n中文")).unwrap().phase,
            Phase::Retired
        );
        assert!(store.queue(&retired, &target, "raw\r\n中文").is_err());
        let received = identity(0);
        assert!(store.queue(&received, &target, "raw\r\n中文").unwrap().1);
        assert!(!store.queue(&received, &target, "raw\r\n中文").unwrap().1);
        assert!(store.queue(&received, &target, "different").is_err());
        store.transition(received.id, Phase::Claimed).unwrap();
        store.transition(received.id, Phase::Unknown).unwrap();
        assert!(!store.queue(&received, &target, "raw\r\n中文").unwrap().1);
        assert!(store.transition(received.id, Phase::Queued).is_err());
    }
    #[test]
    fn disk_failure_blocks_pause_but_stop_still_reaches_the_exact_target() {
        let (_, db, store, target) = setup();
        let effects = std::cell::Cell::new(0);
        db.fail_delivery_write.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(store.control(&identity(0), &target, "pause", || effects.set(1)).is_err());
        assert_eq!(effects.get(), 0);
        let stopped = identity(0);
        db.fail_delivery_write.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(store.control(&stopped, &target, "stop", || effects.set(2)).is_err());
        assert_eq!(effects.get(), 2);
        assert_eq!(store.fence(stopped.id, &target, "stop", None).unwrap().phase, Phase::Unknown);
        assert!(store.control(&stopped, &target, "stop", || panic!("unknown replayed")).is_err());
    }
    #[test]
    fn future_schema_corruption_and_capacity_never_evict_existing_receipts() {
        let (_, db, store, target) = setup();
        let first = identity(0);
        store.queue(&first, &target, "original").unwrap();
        let original = db.get(cf::EXECUTION_STATE, &Store::key(first.id)).unwrap().unwrap();
        db.put(cf::EXECUTION_STATE, &Store::marker(), b"99").unwrap();
        assert!(store.read(first.id).is_err());
        assert_eq!(db.get(cf::EXECUTION_STATE, &Store::key(first.id)).unwrap().unwrap(), original);
        db.put(cf::EXECUTION_STATE, &Store::marker(), b"1").unwrap();
        let mut updates = Vec::new();
        for _ in 1..LIMIT {
            let mut record = store.read(first.id).unwrap().unwrap();
            record.id = Uuid::new_v4();
            updates.push((Store::key(record.id), serde_json::to_vec(&record).unwrap()));
        }
        db.approval_receipt_batch(&updates).unwrap();
        assert!(store.queue(&identity(0), &target, "new").is_err());
        assert_eq!(store.read(first.id).unwrap().unwrap().body.as_deref(), Some("original"));
        db.put(cf::EXECUTION_STATE, &Store::key(first.id), b"{broken").unwrap();
        assert!(store.read(first.id).is_err());
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub schema: u32,
    pub id: Uuid,
    pub owner: String,
    pub target: Target,
    pub operation: String,
    pub revision: u64,
    pub phase: Phase,
    pub body: Option<String>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub id: Uuid,
    pub revision: u64,
}
#[derive(Clone)]
pub struct Store {
    db: Db,
    owner: String,
}
fn storage(error: impl std::fmt::Display) -> metteur_shared::resources::ResourceError {
    fault("storage_unavailable", &error.to_string())
}
impl Store {
    pub fn new(db: Db, owner: &str) -> Result<Self> {
        if owner.is_empty() || owner.len() > 512 {
            return Err(fault("invalid_owner", "An authenticated owner is required"));
        }
        Ok(Self {
            db,
            owner: owner.into(),
        })
    }
    fn key(id: Uuid) -> Vec<u8> {
        [PREFIX, b"records/", id.to_string().as_bytes()].concat()
    }
    fn marker() -> Vec<u8> {
        [PREFIX, b"schema"].concat()
    }
    fn head(target: &Target) -> Result<Vec<u8>> {
        Ok([PREFIX, b"heads/", digest(&serde_json::to_vec(target).map_err(storage)?).as_bytes()]
            .concat())
    }
    fn check(&self) -> Result<()> {
        if self
            .db
            .get(cf::EXECUTION_STATE, &Self::marker())
            .map_err(storage)?
            .is_some_and(|v| v != b"1")
        {
            return Err(fault(
                "unsupported_schema",
                "Workflow receipts require another application version",
            ));
        }
        Ok(())
    }
    pub fn revision(&self, target: &Target) -> Result<u64> {
        self.check()?;
        let value = self.db.get(cf::EXECUTION_STATE, &Self::head(target)?).map_err(storage)?;
        value.map(|v| serde_json::from_slice(&v).map_err(storage)).unwrap_or(Ok(0))
    }
    pub fn read(&self, id: Uuid) -> Result<Option<Receipt>> {
        self.check()?;
        let Some(raw) = self.db.get(cf::EXECUTION_STATE, &Self::key(id)).map_err(storage)? else {
            return Ok(None);
        };
        if raw.len() > 20 * 1024 {
            return Err(fault(
                "corrupt_receipt",
                "Receipt exceeds the supported limit; retain it for review",
            ));
        }
        let record: Receipt = serde_json::from_slice(&raw).map_err(|_| {
            fault("corrupt_receipt", "Receipt is corrupt; work will not be replayed")
        })?;
        if record.schema != 1 || record.id != id || !record.target.valid() {
            return Err(fault(
                "unsupported_schema",
                "Receipt is unsupported; retain it for review",
            ));
        }
        if record.owner != self.owner {
            return Err(fault("permission_denied", "Receipt belongs to another subject"));
        }
        Ok(Some(record))
    }
    fn write(&self, record: &Receipt, advance: bool) -> Result<()> {
        self.check()?;
        let raw = serde_json::to_vec(record).map_err(storage)?;
        let rows = self.db.resource_scan(PREFIX, &[], LIMIT * 2 + 2).map_err(storage)?;
        let key = Self::key(record.id);
        let count =
            rows.iter().filter(|(k, _)| k.starts_with(&[PREFIX, b"records/"].concat())).count();
        let existing = rows.iter().any(|(k, _)| *k == key);
        let total =
            rows.iter().filter(|(k, _)| *k != key).map(|(_, v)| v.len()).sum::<usize>() + raw.len();
        if raw.len() > 20 * 1024
            || (!existing && count >= LIMIT)
            || rows.len() > LIMIT * 2 + 1
            || total > BYTES
        {
            return Err(fault(
                "receipt_capacity",
                "Workflow receipt capacity reached; existing records were retained",
            ));
        }
        let mut updates = vec![(Self::marker(), b"1".to_vec()), (key, raw)];
        if advance {
            updates.push((
                Self::head(&record.target)?,
                serde_json::to_vec(&record.revision).map_err(storage)?,
            ));
        }
        self.db.approval_receipt_batch(&updates).map_err(storage)
    }
    fn exact(record: &Receipt, target: &Target, operation: &str, body: Option<&str>) -> Result<()> {
        if record.target != *target
            || record.operation != operation
            || record.body.as_deref() != body
        {
            return Err(fault(
                "identity_conflict",
                "Receipt identity has different content, operation or target",
            ));
        }
        Ok(())
    }
    /// Hold the same gate as retirement. The callback only sets existing flags;
    /// bus notifications happen after return, avoiding a bus/gate lock inversion.
    pub fn control(
        &self,
        identity: &Identity,
        target: &Target,
        operation: &str,
        apply: impl FnOnce(),
    ) -> Result<Receipt> {
        if identity.id.is_nil()
            || !target.valid()
            || !["stop", "pause", "resume"].contains(&operation)
        {
            return Err(fault(
                "invalid_request",
                "A concrete control identity and target are required",
            ));
        }
        let _gate = self.db.resource_gate.lock().map_err(storage)?;
        if let Some(record) = self.read(identity.id)? {
            Self::exact(&record, target, operation, None)?;
            if record.phase == Phase::Prepared && record.revision != identity.revision {
                return Err(fault(
                    "identity_conflict",
                    "Control identity has a different expected revision",
                ));
            }
            if record.phase != Phase::Prepared {
                return if record.phase == Phase::Accepted {
                    Ok(record)
                } else {
                    Err(fault(
                        "control_unconfirmed",
                        "Inspect the original receipt; this control will not be replayed",
                    ))
                };
            }
        }
        let revision = self.revision(target)?;
        if operation != "stop" && identity.revision != revision {
            return Err(fault(
                "target_changed",
                "A newer control exists; refresh before deciding again",
            ));
        }
        let mut record = Receipt {
            schema: 1,
            id: identity.id,
            owner: self.owner.clone(),
            target: target.clone(),
            operation: operation.into(),
            revision: revision
                .checked_add(1)
                .ok_or_else(|| fault("revision_limit", "Control revision exhausted"))?,
            phase: Phase::Claimed,
            body: None,
        };
        if let Err(error) = self.write(&record, true) {
            // Emergency Stop still reaches its exact live target on disk failure.
            if operation == "stop" {
                apply();
            }
            return Err(error);
        }
        apply();
        record.phase = Phase::Accepted;
        self.write(&record, false)?;
        Ok(record)
    }
    pub fn queue(
        &self,
        identity: &Identity,
        target: &Target,
        body: &str,
    ) -> Result<(Receipt, bool)> {
        let _gate = self.db.resource_gate.lock().map_err(storage)?;
        if let Some(record) = self.read(identity.id)? {
            Self::exact(&record, target, "temporary_input", Some(body))?;
            if record.phase == Phase::Retired {
                return Err(fault("retired", "This input identity was closed; retain the draft"));
            }
            if record.phase != Phase::Prepared {
                return Ok((record, false));
            }
        }
        if identity.id.is_nil()
            || !target.valid()
            || target.kind != "blueprint"
            || body.trim().is_empty()
            || body.len() > 16 * 1024
        {
            return Err(fault(
                "invalid_input",
                "Temporary input requires a concrete run and at most 16 KiB of text",
            ));
        }
        let record = Receipt {
            schema: 1,
            id: identity.id,
            owner: self.owner.clone(),
            target: target.clone(),
            operation: "temporary_input".into(),
            revision: 0,
            phase: Phase::Queued,
            body: Some(body.into()),
        };
        self.write(&record, false)?;
        Ok((record, true))
    }
    pub fn transition(&self, id: Uuid, next: Phase) -> Result<Receipt> {
        let _gate = self.db.resource_gate.lock().map_err(storage)?;
        let mut record = self.read(id)?.ok_or_else(|| fault("not_found", "Receipt not found"))?;
        if record.phase == next {
            return Ok(record);
        }
        if !matches!(
            (&record.phase, &next),
            (Phase::Queued, Phase::Claimed | Phase::Unconsumed)
                | (Phase::Claimed, Phase::Applied | Phase::Unknown)
        ) {
            return Err(fault("invalid_transition", "Receipt cannot be replayed or redirected"));
        }
        record.phase = next;
        self.write(&record, false)?;
        Ok(record)
    }
    pub fn fence(
        &self,
        id: Uuid,
        target: &Target,
        operation: &str,
        body: Option<&str>,
    ) -> Result<Receipt> {
        if id.is_nil()
            || !target.valid()
            || !["stop", "pause", "resume", "temporary_input"].contains(&operation)
            || body.is_some_and(|v| v.len() > 16 * 1024)
        {
            return Err(fault("invalid_request", "Invalid receipt reconciliation"));
        }
        let _gate = self.db.resource_gate.lock().map_err(storage)?;
        if let Some(mut record) = self.read(id)? {
            Self::exact(&record, target, operation, body)?;
            if record.phase == Phase::Prepared {
                record.phase = Phase::Retired;
                self.write(&record, false)?;
            }
            return Ok(record);
        }
        let record = Receipt {
            schema: 1,
            id,
            owner: self.owner.clone(),
            target: target.clone(),
            operation: operation.into(),
            revision: self.revision(target)?,
            phase: if operation == "stop" {
                Phase::Unknown
            } else {
                Phase::Retired
            },
            body: body.map(str::to_owned),
        };
        self.write(&record, false)?;
        Ok(record)
    }
    /// Persist the identity before the narrow mutating RPC. This is a recovery
    /// index, not a queue: no worker reads Prepared records to execute them.
    pub fn prepare(
        &self,
        identity: &Identity,
        target: &Target,
        operation: &str,
        body: Option<&str>,
    ) -> Result<Receipt> {
        if identity.id.is_nil()
            || !target.valid()
            || !["stop", "pause", "resume", "temporary_input"].contains(&operation)
            || (operation == "temporary_input"
                && (target.kind != "blueprint"
                    || body.is_none_or(|v| v.trim().is_empty() || v.len() > 16384)))
            || (operation != "temporary_input" && body.is_some())
        {
            return Err(fault("invalid_request", "Invalid workflow input"));
        }
        let _gate = self.db.resource_gate.lock().map_err(storage)?;
        if let Some(record) = self.read(identity.id)? {
            Self::exact(&record, target, operation, body)?;
            return Ok(record);
        }
        let record = Receipt {
            schema: 1,
            id: identity.id,
            owner: self.owner.clone(),
            target: target.clone(),
            operation: operation.into(),
            revision: identity.revision,
            phase: Phase::Prepared,
            body: body.map(str::to_owned),
        };
        self.write(&record, false)?;
        Ok(record)
    }
    pub fn list(&self, after: Option<Uuid>) -> Result<(Vec<Receipt>, Option<Uuid>)> {
        self.check()?;
        let prefix = [PREFIX, b"records/"].concat();
        let rows = self.db.resource_scan(&prefix, &[], LIMIT + 1).map_err(storage)?;
        if rows.len() > LIMIT {
            return Err(fault("receipt_capacity", "Receipt namespace exceeds supported limits"));
        }
        let mut items = Vec::new();
        let mut cursor = None;
        for (key, raw) in rows {
            let record: Receipt = serde_json::from_slice(&raw).map_err(|_| {
                fault(
                    "corrupt_receipt",
                    "Receipt list contains a corrupt record; export for review",
                )
            })?;
            if key != Self::key(record.id) || record.schema != 1 {
                return Err(fault("corrupt_receipt", "Receipt identity is invalid"));
            }
            if record.owner != self.owner || after.is_some_and(|id| record.id <= id) {
                continue;
            }
            if items.len() == 50 {
                cursor = items.last().map(|r: &Receipt| r.id);
                break;
            }
            items.push(record);
        }
        Ok((items, cursor))
    }
}
