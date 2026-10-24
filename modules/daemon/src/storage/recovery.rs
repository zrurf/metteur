//! Bounded, owner-scoped input recovery, deliberately outside rewind/history.
//! Nothing in a recovered record is an execution permission or a model input.

use super::{
    persistence::{Db, cf},
    resources::{Result, digest, fault},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MAX_RECORD_BYTES: usize = 1024 * 1024;
pub const MAX_SCOPE_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_ACTIVE: usize = 512;
pub const MAX_RECORDS: usize = 4096;
pub const MAX_TOMBSTONES: usize = 128;
pub const MAX_PAGE: usize = 50;
const SCHEMA: u32 = 1;

fn unavailable(_: impl std::fmt::Display) -> metteur_shared::resources::ResourceError {
    fault("storage_unavailable", "Recovery storage is unavailable; input is not backed up")
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DraftInput {
    /// Required for new identities after the first tombstone compaction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creation_epoch: Option<Uuid>,
    pub id: Uuid,
    pub window_id: Uuid,
    pub kind: String,
    pub entity: String,
    pub edit_sequence: u64,
    pub raw: String,
    pub baseline_identity: Option<String>,
    #[serde(default)]
    pub selections: Vec<serde_json::Value>,
    pub pending_submission: Option<Uuid>,
    pub selected_model: Option<String>,
    pub view: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftRecord {
    pub schema_version: u32,
    pub revision: u64,
    pub input: DraftInput,
    pub updated_ms: u64,
    pub discarded: bool,
    pub previous_entity: Option<String>,
    request_id: Uuid,
    request_digest: String,
}

/// List metadata separately, so a page cannot return fifty 1 MiB bodies.
#[derive(Debug, Serialize)]
pub struct DraftMetadata {
    pub id: Uuid,
    pub window_id: Option<Uuid>,
    pub kind: Option<String>,
    pub entity: Option<String>,
    pub revision: Option<u64>,
    pub edit_sequence: Option<u64>,
    pub updated_ms: Option<u64>,
    pub discarded: bool,
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct RecoveryStore {
    db: Db,
    prefix: String,
}

impl RecoveryStore {
    /// `scope` and `subject` must come from the host, never a client owner field.
    pub fn new(db: Db, scope: &str, subject: &str) -> Result<Self> {
        if scope.is_empty() || subject.is_empty() {
            return Err(fault(
                "invalid_scope",
                "Recovery requires a host-resolved scope and owner",
            ));
        }
        let identity = serde_json::to_vec(&(scope, subject)).map_err(unavailable)?;
        Ok(Self {
            db,
            prefix: format!("dw/recovery/{}/", digest(&identity)),
        })
    }

    fn marker(&self) -> Vec<u8> {
        format!("{}schema", self.prefix).into_bytes()
    }
    fn key(&self, id: Uuid) -> Vec<u8> {
        format!("{}records/{id}", self.prefix).into_bytes()
    }
    fn records_prefix(&self) -> Vec<u8> {
        format!("{}records/", self.prefix).into_bytes()
    }
    fn epoch_key(&self) -> Vec<u8> {
        format!("{}creation_epoch", self.prefix).into_bytes()
    }
    pub fn creation_epoch(&self) -> Result<Option<Uuid>> {
        self.check_schema()?;
        self.db
            .get(cf::EXECUTION_STATE, &self.epoch_key())
            .map_err(unavailable)?
            .map(|raw| Uuid::from_slice(&raw).map_err(unavailable))
            .transpose()
    }
    fn check_schema(&self) -> Result<()> {
        if self
            .db
            .get(cf::EXECUTION_STATE, &self.marker())
            .map_err(unavailable)?
            .is_some_and(|value| value != b"1" && value != b"2")
        {
            return Err(fault(
                "unsupported_schema",
                "Recovery schema requires a different application version",
            ));
        }
        Ok(())
    }
    fn decode(raw: &[u8]) -> Result<DraftRecord> {
        let record: DraftRecord = serde_json::from_slice(raw).map_err(|_| {
            fault("corrupt_record", "Recovery record is quarantined; export it before repair")
        })?;
        if record.schema_version != SCHEMA {
            return Err(fault(
                "unsupported_schema",
                "Unknown recovery record schema; retained without modification",
            ));
        }
        Ok(record)
    }
    pub fn get(&self, id: Uuid) -> Result<Option<DraftRecord>> {
        self.check_schema()?;
        self.db
            .get(cf::EXECUTION_STATE, &self.key(id))
            .map_err(unavailable)?
            .map(|raw| Self::decode(&raw))
            .transpose()
    }
    /// One bounded metadata selection, avoiding client-side paginated startup
    /// reads. Corrupt/future records remain quarantined and are never rewritten.
    pub fn latest_view(&self) -> Result<Option<DraftRecord>> {
        self.check_schema()?;
        let rows = self
            .db
            .resource_scan(&self.records_prefix(), &[], MAX_RECORDS + 1)
            .map_err(unavailable)?;
        if rows.len() > MAX_RECORDS {
            return Err(fault(
                "record_limit",
                "Recovery metadata exceeds the supported view limit",
            ));
        }
        Ok(rows
            .into_iter()
            .filter_map(|(_, raw)| Self::decode(&raw).ok())
            .filter(|record| {
                !record.discarded
                    && record.input.kind == "view_state"
                    && record.input.entity == "workspace"
            })
            .max_by_key(|record| (record.updated_ms, record.input.id)))
    }
    /// Pending manual inputs can outlive a browser. Return identities only;
    /// their raw recovery copies remain untouched and corrupt rows isolated.
    pub fn pending_submissions(&self) -> Result<Vec<Uuid>> {
        self.check_schema()?;
        let rows = self
            .db
            .resource_scan(&self.records_prefix(), &[], MAX_RECORDS + 1)
            .map_err(unavailable)?;
        let mut pending = std::collections::BTreeSet::new();
        for (_, raw) in rows {
            if let Ok(record) = Self::decode(&raw)
                && !record.discarded
                && matches!(record.input.kind.as_str(), "chat" | "new_chat")
                && let Some(id) = record.input.pending_submission
            {
                pending.insert(id);
            }
        }
        Ok(pending.into_iter().collect())
    }
    pub fn pending_supervision(&self) -> Result<Vec<(Uuid, Uuid)>> {
        self.check_schema()?;
        let rows = self
            .db
            .resource_scan(&self.records_prefix(), &[], MAX_RECORDS + 1)
            .map_err(unavailable)?;
        let mut pending = std::collections::BTreeSet::new();
        for (_, raw) in rows {
            if let Ok(record) = Self::decode(&raw)
                && !record.discarded
                && record.input.kind == "supervision"
                && let Some(id) = record.input.pending_submission
                && let Ok(run) = Uuid::parse_str(&record.input.entity)
            {
                pending.insert((run, id));
            }
        }
        Ok(pending.into_iter().collect())
    }
    /// Export owned bytes without decoding them, including quarantined records.
    pub fn export(&self, id: Uuid) -> Result<Vec<u8>> {
        let raw = self
            .db
            .get(cf::EXECUTION_STATE, &self.key(id))
            .map_err(unavailable)?
            .ok_or_else(|| fault("not_found", "Recovery copy not found"))?;
        if raw.len() > MAX_RECORD_BYTES {
            return Err(fault(
                "record_limit",
                "Corrupt recovery record exceeds export RPC limit; retain the database for manual recovery",
            ));
        }
        Ok(raw)
    }
    pub fn list(&self, after: Option<Uuid>, limit: usize) -> Result<Vec<DraftMetadata>> {
        self.check_schema()?;
        let prefix = self.records_prefix();
        let after = after.map(|id| self.key(id)).unwrap_or_default();
        self.db
            .resource_scan(&prefix, &after, limit.clamp(1, MAX_PAGE))
            .map_err(unavailable)?
            .into_iter()
            .map(|(key, raw)| {
                let id = std::str::from_utf8(&key[prefix.len()..])
                    .ok()
                    .and_then(|key| Uuid::parse_str(key).ok())
                    .ok_or_else(|| {
                        fault("corrupt_record", "Invalid recovery key; retained for manual repair")
                    })?;
                Ok(match Self::decode(&raw) {
                    Ok(record) => DraftMetadata {
                        id,
                        window_id: Some(record.input.window_id),
                        kind: Some(record.input.kind),
                        entity: Some(record.input.entity),
                        revision: Some(record.revision),
                        edit_sequence: Some(record.input.edit_sequence),
                        updated_ms: Some(record.updated_ms),
                        discarded: record.discarded,
                        error: None,
                    },
                    Err(error) => DraftMetadata {
                        id,
                        window_id: None,
                        kind: None,
                        entity: None,
                        revision: None,
                        edit_sequence: None,
                        updated_ms: None,
                        discarded: false,
                        error: Some(error.code),
                    },
                })
            })
            .collect()
    }
    fn check_input(input: &DraftInput) -> Result<()> {
        if !matches!(
            input.kind.as_str(),
            "text" | "canvas" | "chat" | "new_chat" | "supervision" | "view_state" | "workflow" | "workflow_review"
        ) || input.entity.is_empty()
            || input.entity.len() > 2048
            || input.selections.len() > 64
            || input.baseline_identity.as_ref().is_some_and(|v| v.len() > 512)
            || input.selected_model.as_ref().is_some_and(|v| v.len() > 512)
        {
            return Err(fault("invalid_request", "Invalid recovery identity or selection limits"));
        }
        if serde_json::to_vec(input).map_err(unavailable)?.len() > MAX_RECORD_BYTES - 1024 {
            return Err(fault(
                "record_limit",
                "Draft exceeds the recovery limit; no content was truncated",
            ));
        }
        Ok(())
    }
    fn conflict(expected: u64, actual: u64) -> metteur_shared::resources::ResourceError {
        let mut error =
            fault("revision_conflict", "Recovery copy changed; retain local input and reconcile");
        error.expected_revision = Some(expected);
        error.actual_revision = Some(actual);
        error
    }

    /// One durable record replaces the previous body. No append-only history or
    /// per-keystroke receipt log. A stale retry is rejected rather than replayed.
    pub fn backup(
        &self,
        input: DraftInput,
        expected: u64,
        request: Uuid,
        now: u64,
    ) -> Result<DraftRecord> {
        Self::check_input(&input)?;
        let fingerprint =
            digest(&serde_json::to_vec(&("backup", expected, &input)).map_err(unavailable)?);
        let _guard = self.db.resource_gate.lock().map_err(unavailable)?;
        self.check_schema()?;
        let old = self.get(input.id)?;
        if old.is_none() && self.creation_epoch()? != input.creation_epoch {
            return Err(fault(
                "creation_epoch_required",
                "Recovery generation changed; refresh before creating a new copy",
            ));
        }
        if let Some(previous) = &old {
            if previous.request_id == request {
                if previous.request_digest != fingerprint {
                    return Err(fault(
                        "request_conflict",
                        "Recovery request ID has different content",
                    ));
                }
                return Ok(previous.clone());
            }
            if previous.discarded {
                return Err(fault("discarded", "Draft was discarded; late writes are rejected"));
            }
            if previous.input.window_id != input.window_id
                || previous.input.creation_epoch != input.creation_epoch
                || previous.input.kind != input.kind
                || previous.input.entity != input.entity
            {
                return Err(fault(
                    "identity_conflict",
                    "Draft identity changed; use a separate recovery copy",
                ));
            }
            if input.edit_sequence <= previous.input.edit_sequence {
                return Err(fault(
                    "stale_edit",
                    "Only a newer edit sequence can replace the recovery copy",
                ));
            }
        }
        let actual = old.as_ref().map_or(0, |record| record.revision);
        if actual != expected {
            return Err(Self::conflict(expected, actual));
        }
        let revision = actual
            .checked_add(1)
            .ok_or_else(|| fault("revision_exhausted", "Recovery revision exhausted"))?;
        let record = DraftRecord {
            schema_version: SCHEMA,
            revision,
            input,
            updated_ms: now,
            discarded: false,
            previous_entity: old.and_then(|r| r.previous_entity),
            request_id: request,
            request_digest: fingerprint,
        };
        self.commit(&record)?;
        Ok(record)
    }

    /// `saved_sequence` makes clearing a successfully saved old snapshot fail
    /// safely when a newer edit has already been backed up.
    pub fn discard(
        &self,
        id: Uuid,
        expected: u64,
        saved_sequence: u64,
        request: Uuid,
        now: u64,
    ) -> Result<DraftRecord> {
        let fingerprint = digest(
            &serde_json::to_vec(&("discard", id, expected, saved_sequence)).map_err(unavailable)?,
        );
        let _guard = self.db.resource_gate.lock().map_err(unavailable)?;
        let mut record = self.get(id)?.ok_or_else(|| {
            if expected > 0 && self.creation_epoch().ok().flatten().is_some() {
                // Active records are never reclaimed. A missing previously
                // acknowledged copy is therefore already retired.
                fault("retired", "Recovery copy was already discarded and compacted")
            } else {
                fault("not_found", "Recovery copy not found")
            }
        })?;
        if record.request_id == request {
            if record.request_digest != fingerprint {
                return Err(fault("request_conflict", "Recovery request ID has different content"));
            }
            return Ok(record);
        }
        if record.discarded {
            return Err(fault("discarded", "Recovery copy is already discarded"));
        }
        if record.revision != expected {
            return Err(Self::conflict(expected, record.revision));
        }
        if record.input.edit_sequence != saved_sequence {
            return Err(fault(
                "stale_edit",
                "Newer input is still recoverable; it was not cleared",
            ));
        }
        record.revision = record
            .revision
            .checked_add(1)
            .ok_or_else(|| fault("revision_exhausted", "Recovery revision exhausted"))?;
        record.discarded = true;
        record.updated_ms = now;
        record.input.raw.clear();
        record.input.selections.clear();
        record.input.view = None;
        record.input.baseline_identity = None;
        record.input.pending_submission = None;
        record.input.selected_model = None;
        record.request_id = request;
        record.request_digest = fingerprint;
        self.commit(&record)?;
        Ok(record)
    }

    /// The host calls this only after confirming a filesystem rename. A
    /// durable transfer changes identity/CAS together; late old-path writes
    /// fail. A directory rename transfers its children without guessing names.
    pub fn rename_entity(&self, from: &str, to: &str, now: u64) -> Result<usize> {
        let _guard = self.db.resource_gate.lock().map_err(unavailable)?;
        self.check_schema()?;
        let rows = self
            .db
            .resource_scan(&self.records_prefix(), &[], MAX_RECORDS + 1)
            .map_err(unavailable)?;
        let mut writes = Vec::new();
        let mut total = rows.iter().map(|(_, raw)| raw.len()).sum::<usize>();
        for (key, raw) in rows {
            let Ok(mut record) = Self::decode(&raw) else {
                continue;
            };
            if record.discarded || !matches!(record.input.kind.as_str(), "text" | "canvas") {
                continue;
            }
            let suffix = if record.input.entity == from {
                Some("")
            } else {
                record.input.entity.strip_prefix(from).filter(|tail| tail.starts_with('/'))
            };
            if let Some(suffix) = suffix {
                let target = format!("{to}{suffix}");
                if target.len() > 2048 {
                    return Err(fault(
                        "invalid_target",
                        "Renamed recovery identity is too long; original copy retained",
                    ));
                }
                record.previous_entity = Some(record.input.entity.clone());
                record.input.entity = target;
                record.revision = record
                    .revision
                    .checked_add(1)
                    .ok_or_else(|| fault("revision_exhausted", "Recovery revision exhausted"))?;
                record.updated_ms = now;
                record.request_id = Uuid::new_v4();
                record.request_digest = digest(format!("rename:{from}:{to}").as_bytes());
                let bytes = serde_json::to_vec(&record).map_err(unavailable)?;
                if bytes.len() > MAX_RECORD_BYTES {
                    return Err(fault(
                        "record_limit",
                        "Rename metadata exceeds recovery limit; original copy retained",
                    ));
                }
                total = total.saturating_sub(raw.len()).saturating_add(bytes.len());
                if total > MAX_SCOPE_BYTES {
                    return Err(fault(
                        "scope_capacity",
                        "Rename metadata exceeds recovery capacity; original copies retained",
                    ));
                }
                writes.push((key, bytes));
            }
        }
        let count = writes.len();
        if count > 0 {
            self.db.recovery_batch(&writes).map_err(unavailable)?;
        }
        Ok(count)
    }

    fn commit(&self, record: &DraftRecord) -> Result<()> {
        let bytes = serde_json::to_vec(record).map_err(unavailable)?;
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(fault(
                "record_limit",
                "Draft exceeds the recovery limit; input was retained",
            ));
        }
        let key = self.key(record.input.id);
        let rows = self
            .db
            .resource_scan(&self.records_prefix(), &[], MAX_RECORDS + 1)
            .map_err(unavailable)?;
        let mut retired: Vec<_> = rows
            .iter()
            .filter_map(|(key, raw)| {
                let other = Self::decode(raw).ok()?;
                (other.discarded && *key != self.key(record.input.id))
                    .then_some((other.updated_ms, key.clone()))
            })
            .collect();
        retired.sort();
        let excess = retired.len().saturating_sub(MAX_TOMBSTONES - usize::from(record.discarded));
        let deletes: std::collections::BTreeSet<_> =
            retired.into_iter().take(excess).map(|(_, key)| key).collect();
        let mut total = bytes.len();
        let mut count = 1;
        let mut active = usize::from(!record.discarded);
        for (other_key, raw) in rows {
            if other_key == key || deletes.contains(&other_key) {
                continue;
            }
            count += 1;
            total = total.saturating_add(raw.len());
            // Corrupt/unknown records keep their capacity reservation.
            if Self::decode(&raw).map_or(true, |other| !other.discarded) {
                active += 1;
            }
        }
        if total > MAX_SCOPE_BYTES || count > MAX_RECORDS || active > MAX_ACTIVE {
            return Err(fault(
                "scope_capacity",
                "Recovery capacity reached; existing copies were retained",
            ));
        }
        let compacted = !deletes.is_empty();
        let version = if compacted || self.creation_epoch()?.is_some() {
            b"2"
        } else {
            b"1"
        };
        let mut writes = vec![(self.marker(), version.to_vec()), (key, bytes)];
        if compacted {
            writes.push((self.epoch_key(), Uuid::new_v4().as_bytes().to_vec()));
        }
        self.db
            .recovery_compact_batch(&writes, &deletes.into_iter().collect::<Vec<_>>())
            .map_err(unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn setup() -> (std::path::PathBuf, Db, RecoveryStore) {
        let root = std::env::temp_dir().join(format!("dw-recovery-{}", Uuid::new_v4()));
        let db = Db::open(&root).unwrap();
        let store = RecoveryStore::new(db.clone(), "project:fixture", "local").unwrap();
        (root, db, store)
    }
    fn input() -> DraftInput {
        DraftInput {
            creation_epoch: None,
            id: Uuid::new_v4(),
            window_id: Uuid::new_v4(),
            kind: "canvas".into(),
            entity: "repair.blueprint".into(),
            edit_sequence: 1,
            raw: "{invalid\r\n中文".into(),
            baseline_identity: Some("sha256:baseline".into()),
            selections: vec![
                serde_json::json!({"scope":"personal","id":"p"}),
                serde_json::json!({"scope":"project","id":"p"}),
                serde_json::json!({"scope":"personal","id":"p"}),
            ],
            pending_submission: Some(Uuid::new_v4()),
            selected_model: Some("chosen-model".into()),
            view: Some(serde_json::json!({"cursor":17})),
        }
    }

    #[test]
    fn saved_copy_churn_reclaims_capacity_without_reviving_retired_ids() {
        let (root, db, store) = setup();
        let live = input();
        store.backup(live.clone(), 0, Uuid::new_v4(), 1).unwrap();
        let old = input();
        store.backup(old.clone(), 0, Uuid::new_v4(), 2).unwrap();
        let retired = store.discard(old.id, 1, 1, Uuid::new_v4(), 3).unwrap();
        // A legacy scope already full of tombstones must repair on its next
        // ordinary write; no deletion of active/corrupt copies is allowed.
        let mut seed = Vec::new();
        for n in 0..MAX_RECORDS - 2 {
            let mut row = retired.clone();
            row.input.id = Uuid::new_v4();
            row.updated_ms = 4 + n as u64;
            seed.push((store.key(row.input.id), serde_json::to_vec(&row).unwrap()));
        }
        db.recovery_batch(&seed).unwrap();
        for n in 0..MAX_TOMBSTONES * 2 {
            let mut draft = input();
            draft.creation_epoch = store.creation_epoch().unwrap();
            store.backup(draft.clone(), 0, Uuid::new_v4(), 10_000 + n as u64).unwrap();
            store.discard(draft.id, 1, 1, Uuid::new_v4(), 20_000 + n as u64).unwrap();
        }
        assert!(
            db.resource_scan(&store.records_prefix(), &[], MAX_RECORDS + 1).unwrap().len()
                <= MAX_TOMBSTONES + 1
        );
        assert_eq!(store.get(live.id).unwrap().unwrap().input.raw, live.raw);
        assert!(store.get(old.id).unwrap().is_none());
        assert_eq!(
            store.backup(old.clone(), 0, Uuid::new_v4(), 30_000).unwrap_err().code,
            "creation_epoch_required"
        );
        assert_eq!(
            store.discard(old.id, 1, 1, Uuid::new_v4(), 30_000).unwrap_err().code,
            "retired"
        );
        let mut newer = live.clone();
        newer.edit_sequence += 1;
        newer.raw = "still editable after compaction".into();
        store.backup(newer, 1, Uuid::new_v4(), 30_001).unwrap();
        drop(store);
        drop(db);
        let reopened =
            RecoveryStore::new(Db::open(&root).unwrap(), "project:fixture", "local").unwrap();
        assert_eq!(
            reopened.backup(old, 0, Uuid::new_v4(), 30_002).unwrap_err().code,
            "creation_epoch_required"
        );
        assert_eq!(
            reopened.get(live.id).unwrap().unwrap().input.raw,
            "still editable after compaction"
        );
    }

    #[test]
    fn failed_compaction_keeps_old_generation_and_all_records() {
        let (_, db, store) = setup();
        for n in 0..MAX_TOMBSTONES {
            let draft = input();
            store.backup(draft.clone(), 0, Uuid::new_v4(), n as u64).unwrap();
            store.discard(draft.id, 1, 1, Uuid::new_v4(), n as u64).unwrap();
        }
        let draft = input();
        store.backup(draft.clone(), 0, Uuid::new_v4(), 1000).unwrap();
        db.fail_recovery_write.store(true, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            store.discard(draft.id, 1, 1, Uuid::new_v4(), 1001).unwrap_err().code,
            "storage_unavailable"
        );
        assert_eq!(store.creation_epoch().unwrap(), None);
        assert!(!store.get(draft.id).unwrap().unwrap().discarded);
        assert_eq!(
            db.resource_scan(&store.records_prefix(), &[], MAX_RECORDS + 1).unwrap().len(),
            MAX_TOMBSTONES + 1
        );
    }

    #[test]
    fn ack_survives_reopen_preserves_invalid_raw_order_and_pending_identity() {
        let (root, db, store) = setup();
        let draft = input();
        store.backup(draft.clone(), 0, Uuid::new_v4(), 1).unwrap();
        drop(store);
        drop(db);
        let db = Db::open(&root).unwrap();
        let store = RecoveryStore::new(db, "project:fixture", "local").unwrap();
        assert_eq!(store.get(draft.id).unwrap().unwrap().input, draft);
    }

    #[test]
    fn latest_view_is_bounded_owner_scoped_read_only_and_durable() {
        let (root, db, store) = setup();
        let mut view = input();
        view.kind = "view_state".into();
        view.entity = "workspace".into();
        view.raw.clear();
        view.pending_submission = None;
        view.selected_model = None;
        view.selections.clear();
        view.view = Some(
            serde_json::json!({"schema":1,"session":Uuid::new_v4(),"tabs":[{"surface":"chat"}]}),
        );
        assert!(store.latest_view().unwrap().is_none());
        store.backup(view.clone(), 0, Uuid::new_v4(), 10).unwrap();
        let mut newer = view.clone();
        newer.id = Uuid::new_v4();
        newer.window_id = Uuid::new_v4();
        store.backup(newer.clone(), 0, Uuid::new_v4(), 20).unwrap();
        store.backup(input(), 0, Uuid::new_v4(), 100).unwrap();
        let other = RecoveryStore::new(db.clone(), "project:fixture", "other-owner").unwrap();
        other.backup(newer.clone(), 0, Uuid::new_v4(), 200).unwrap();
        assert_eq!(store.latest_view().unwrap().unwrap().input.id, newer.id);
        store.discard(newer.id, 1, 1, Uuid::new_v4(), 30).unwrap();
        let mut corrupt = view.clone();
        corrupt.id = Uuid::new_v4();
        db.put(cf::EXECUTION_STATE, &store.key(corrupt.id), b"{invalid").unwrap();
        let before = store.list(None, MAX_PAGE).unwrap().len();
        assert_eq!(store.latest_view().unwrap().unwrap().input, view);
        assert_eq!(store.list(None, MAX_PAGE).unwrap().len(), before);
        assert!(store.export(corrupt.id).unwrap().starts_with(b"{invalid"));
        drop(other);
        drop(store);
        drop(db);
        let reopened =
            RecoveryStore::new(Db::open(&root).unwrap(), "project:fixture", "local").unwrap();
        assert_eq!(reopened.latest_view().unwrap().unwrap().input, view);
    }

    #[test]
    fn replay_and_cas_reject_different_content_old_edits_and_wrong_window() {
        let (_, _, store) = setup();
        let mut draft = input();
        let request = Uuid::new_v4();
        let first = store.backup(draft.clone(), 0, request, 1).unwrap();
        assert_eq!(store.backup(draft.clone(), 0, request, 2).unwrap().revision, first.revision);
        draft.raw.push('x');
        assert_eq!(
            store.backup(draft.clone(), 0, request, 3).unwrap_err().code,
            "request_conflict"
        );
        assert_eq!(
            store.backup(draft.clone(), 1, Uuid::new_v4(), 3).unwrap_err().code,
            "stale_edit"
        );
        draft.edit_sequence += 1;
        assert_eq!(
            store.backup(draft.clone(), 0, Uuid::new_v4(), 3).unwrap_err().code,
            "revision_conflict"
        );
        draft.window_id = Uuid::new_v4();
        assert_eq!(
            store.backup(draft, 1, Uuid::new_v4(), 3).unwrap_err().code,
            "identity_conflict"
        );
    }

    #[test]
    fn save_clears_only_its_edit_and_tombstone_fences_late_backup() {
        let (_, db, store) = setup();
        let mut draft = input();
        store.backup(draft.clone(), 0, Uuid::new_v4(), 1).unwrap();
        draft.edit_sequence = 2;
        draft.raw.push_str("new input");
        store.backup(draft.clone(), 1, Uuid::new_v4(), 2).unwrap();
        assert!(store.discard(draft.id, 2, 1, Uuid::new_v4(), 3).is_err());
        assert_eq!(store.get(draft.id).unwrap().unwrap().input.raw, draft.raw);
        let request = Uuid::new_v4();
        store.discard(draft.id, 2, 2, request, 3).unwrap();
        assert!(store.discard(draft.id, 2, 2, request, 4).unwrap().discarded);
        draft.edit_sequence = 3;
        assert_eq!(
            store.backup(draft.clone(), 3, Uuid::new_v4(), 4).unwrap_err().code,
            "discarded"
        );
        db.commit_chat_rewind(Uuid::new_v4().as_bytes(), b"{}", &[]).unwrap();
        let record = store.get(draft.id).unwrap().unwrap();
        assert!(record.discarded);
        assert_eq!(record.input.raw, "");
    }

    #[test]
    fn owners_scopes_and_windows_have_independent_copies() {
        let (_, db, store) = setup();
        let draft = input();
        store.backup(draft.clone(), 0, Uuid::new_v4(), 1).unwrap();
        for (scope, owner) in
            [("project:fixture", "other"), ("personal", "local"), ("project:other", "local")]
        {
            let other = RecoveryStore::new(db.clone(), scope, owner).unwrap();
            assert!(other.get(draft.id).unwrap().is_none());
            assert!(other.list(None, 50).unwrap().is_empty());
        }
        let mut second = draft.clone();
        second.id = Uuid::new_v4();
        second.window_id = Uuid::new_v4();
        second.raw = "other window".into();
        store.backup(second.clone(), 0, Uuid::new_v4(), 2).unwrap();
        assert_eq!(store.list(None, 50).unwrap().len(), 2);
        assert_eq!(store.get(draft.id).unwrap().unwrap().input.raw, draft.raw);
    }

    #[test]
    fn failed_durable_write_never_acknowledges_or_replaces_last_copy() {
        let (_, db, store) = setup();
        let mut draft = input();
        store.backup(draft.clone(), 0, Uuid::new_v4(), 1).unwrap();
        let old = draft.raw.clone();
        draft.edit_sequence = 2;
        draft.raw = "newer".into();
        db.fail_recovery_write.store(true, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            store.backup(draft.clone(), 1, Uuid::new_v4(), 2).unwrap_err().code,
            "storage_unavailable"
        );
        assert_eq!(store.get(draft.id).unwrap().unwrap().input.raw, old);
        draft.raw = "x".repeat(MAX_RECORD_BYTES);
        assert_eq!(
            store.backup(draft.clone(), 1, Uuid::new_v4(), 2).unwrap_err().code,
            "record_limit"
        );
        assert_eq!(store.get(draft.id).unwrap().unwrap().revision, 1);
    }

    #[test]
    fn corrupt_and_unknown_schema_are_retained_and_isolated() {
        let (_, db, store) = setup();
        let draft = input();
        store.backup(draft.clone(), 0, Uuid::new_v4(), 1).unwrap();
        let broken = Uuid::new_v4();
        db.put_durable(cf::EXECUTION_STATE, &store.key(broken), b"broken raw").unwrap();
        let page = store.list(None, 50).unwrap();
        assert_eq!(page.len(), 2);
        assert!(page.iter().any(|item| item.error.as_deref() == Some("corrupt_record")));
        assert_eq!(store.get(broken).unwrap_err().code, "corrupt_record");
        assert_eq!(store.export(broken).unwrap(), b"broken raw");
        assert_eq!(
            db.get(cf::EXECUTION_STATE, &store.key(broken)).unwrap().unwrap(),
            b"broken raw"
        );
        db.put_durable(cf::EXECUTION_STATE, &store.marker(), b"999").unwrap();
        assert_eq!(store.list(None, 50).unwrap_err().code, "unsupported_schema");
        assert_eq!(store.export(broken).unwrap(), b"broken raw");
        assert!(store.backup(input(), 0, Uuid::new_v4(), 2).is_err());
    }

    #[test]
    fn repeated_backups_are_bounded_and_do_not_broadcast_business_changes() {
        let (_, db, store) = setup();
        let changes = db.subscribe_changes();
        let mut draft = input();
        for sequence in 1..=1000 {
            draft.edit_sequence = sequence;
            draft.raw = format!("draft {sequence}");
            store.backup(draft.clone(), sequence - 1, Uuid::new_v4(), sequence).unwrap();
        }
        assert_eq!(store.get(draft.id).unwrap().unwrap().input.edit_sequence, 1000);
        assert!(!changes.has_changed().unwrap());
        let rows = db.resource_scan(store.prefix.as_bytes(), &[], 10_000).unwrap();
        assert_eq!(
            rows.len(),
            2,
            "one schema marker and one current record, without history/receipt growth"
        );
        let metadata = serde_json::to_string(&store.list(None, 50).unwrap()).unwrap();
        assert!(!metadata.contains("draft 1000"));
    }

    #[test]
    fn active_capacity_failure_keeps_every_existing_copy() {
        let (_, _, store) = setup();
        let mut ids = Vec::new();
        for _ in 0..MAX_ACTIVE {
            let draft = input();
            ids.push(draft.id);
            store.backup(draft, 0, Uuid::new_v4(), 1).unwrap();
        }
        assert_eq!(store.backup(input(), 0, Uuid::new_v4(), 1).unwrap_err().code, "scope_capacity");
        for id in ids {
            assert!(store.get(id).unwrap().is_some());
        }
        assert_eq!(store.list(None, 10_000).unwrap().len(), MAX_PAGE);
    }

    #[test]
    fn confirmed_rename_transfers_copies_and_fences_old_path_writes() {
        let (_, _, store) = setup();
        let mut first = input();
        first.entity = "src/a.ts".into();
        first.kind = "text".into();
        let mut second = first.clone();
        second.id = Uuid::new_v4();
        second.window_id = Uuid::new_v4();
        let mut sibling = first.clone();
        sibling.id = Uuid::new_v4();
        sibling.entity = "src-other/a.ts".into();
        for draft in [first.clone(), second.clone(), sibling.clone()] {
            store.backup(draft, 0, Uuid::new_v4(), 1).unwrap();
        }
        assert_eq!(store.rename_entity("src", "renamed", 2).unwrap(), 2);
        for draft in [first.clone(), second] {
            let record = store.get(draft.id).unwrap().unwrap();
            assert_eq!(record.input.entity, "renamed/a.ts");
            assert_eq!(record.revision, 2);
            assert_eq!(record.previous_entity.as_deref(), Some("src/a.ts"));
        }
        first.edit_sequence = 2;
        assert_eq!(
            store.backup(first, 1, Uuid::new_v4(), 3).unwrap_err().code,
            "identity_conflict"
        );
        assert_eq!(store.get(sibling.id).unwrap().unwrap().input.entity, "src-other/a.ts");
    }

    #[test]
    fn rename_cannot_exceed_scope_capacity_or_partially_transfer_copies() {
        let (_, db, store) = setup();
        let draft = input();
        let record = store.backup(draft.clone(), 0, Uuid::new_v4(), 1).unwrap();
        let bytes = serde_json::to_vec(&record).unwrap();
        // A damaged record retains its capacity reservation. Use one directly
        // seeded value to reach the boundary without creating 64 MiB of drafts.
        db.recovery_batch(&[(
            store.key(Uuid::new_v4()),
            vec![b'x'; MAX_SCOPE_BYTES - bytes.len()],
        )])
        .unwrap();
        assert_eq!(
            store.rename_entity(&draft.entity, "longer-renamed.blueprint", 2).unwrap_err().code,
            "scope_capacity"
        );
        let unchanged = store.get(draft.id).unwrap().unwrap();
        assert_eq!(unchanged.revision, 1);
        assert_eq!(unchanged.input.entity, draft.entity);
    }
}
