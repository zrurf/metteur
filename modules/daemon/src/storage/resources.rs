//! Append-only resource revisions, outside rewindable execution snapshots.

use std::path::Path;
use metteur_shared::resources::{ResourceError, ResourceScope, SCHEMA_VERSION};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use super::persistence::{Db, cf};

pub type Result<T> = std::result::Result<T, ResourceError>;
const MARKER: &[u8] = b"nx/resources/schema";

pub fn fault(code: &str, message: &str) -> ResourceError {
    ResourceError { code: code.into(), message: message.into(), expected_revision: None, actual_revision: None, current_digest: None }
}
pub fn digest(bytes: &[u8]) -> String { Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect() }
fn storage_error(_: impl std::fmt::Display) -> ResourceError { fault("storage_unavailable", "Resource storage is unavailable") }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredRecord {
    pub schema_version: u32,
    pub revision: u64,
    pub content_digest: String,
    pub value: serde_json::Value,
}

#[derive(Clone)]
pub struct ResourceStore { db: Db, prefix: String }

impl ResourceStore {
    pub fn new(db: Db, scope: &ResourceScope, namespace: &str) -> Result<Self> {
        if !matches!(namespace, "memory" | "prompts" | "snapshots" | "continuations" | "prompt_intents") {
            return Err(fault("invalid_namespace", "Unknown resource namespace"));
        }
        let owner = digest(&serde_json::to_vec(scope).map_err(storage_error)?);
        Ok(Self { db, prefix: format!("nx/v1/{owner}/{namespace}/") })
    }

    /// None is a legacy database; inspecting it never creates a marker or task.
    pub fn inspect(db: &Db) -> Result<Option<u32>> {
        db.get(cf::EXECUTION_STATE, MARKER).map_err(storage_error)?
            .map(|raw| serde_json::from_slice(&raw).map_err(storage_error)).transpose()
    }

    /// Additive migration with a consistent pre-migration checkpoint.
    pub fn prepare(db: &Db, backup: &Path) -> Result<()> {
        let _guard = db.resource_gate.lock().map_err(storage_error)?;
        match Self::inspect(db)? {
            Some(SCHEMA_VERSION) => Ok(()),
            Some(_) => Err(fault("unsupported_schema", "Resource schema requires a different application version")),
            None => {
                if !backup.exists() { db.checkpoint(backup).map_err(storage_error)?; }
                db.put_durable(cf::EXECUTION_STATE, MARKER, b"1").map_err(storage_error)
            }
        }
    }

    fn check_schema(&self) -> Result<()> {
        if Self::inspect(&self.db)? != Some(SCHEMA_VERSION) {
            return Err(fault("unsupported_schema", "Prepare the resource schema before writing"));
        }
        Ok(())
    }
    fn key(&self, id: uuid::Uuid) -> String { format!("{}current/{id}", self.prefix) }
    fn decode(raw: &[u8]) -> Result<StoredRecord> {
        let record: StoredRecord = serde_json::from_slice(raw).map_err(storage_error)?;
        if record.schema_version != SCHEMA_VERSION { return Err(fault("unsupported_schema", "Unknown record schema")); }
        Ok(record)
    }
    pub fn get(&self, id: uuid::Uuid) -> Result<Option<StoredRecord>> {
        self.db.get(cf::EXECUTION_STATE, self.key(id).as_bytes()).map_err(storage_error)?
            .map(|raw| Self::decode(&raw)).transpose()
    }
    pub fn revision(&self, id: uuid::Uuid, revision: u64) -> Result<Option<StoredRecord>> {
        self.db.get(cf::EXECUTION_STATE, format!("{}history/{id}/{revision:020}", self.prefix).as_bytes()).map_err(storage_error)?
            .map(|raw| Self::decode(&raw)).transpose()
    }
    pub fn list(&self, after: Option<uuid::Uuid>, limit: usize) -> Result<Vec<(uuid::Uuid, StoredRecord)>> {
        let prefix = format!("{}current/", self.prefix);
        let after = after.map(|id| self.key(id)).unwrap_or_default();
        self.db.resource_scan(prefix.as_bytes(), after.as_bytes(), limit.clamp(1, 50)).map_err(storage_error)?
            .into_iter().map(|(key, raw)| {
                let key = std::str::from_utf8(&key).map_err(storage_error)?;
                let id = uuid::Uuid::parse_str(&key[prefix.len()..]).map_err(storage_error)?;
                Ok((id, Self::decode(&raw)?))
            }).collect()
    }

    /// CAS and replay receipts share the same lock and atomic durable batch.
    pub fn write(&self, id: uuid::Uuid, expected: u64, request_id: uuid::Uuid, value: serde_json::Value) -> Result<StoredRecord> {
        let _guard = self.db.resource_gate.lock().map_err(storage_error)?;
        self.check_schema()?;
        let fingerprint = digest(&serde_json::to_vec(&(id, expected, &value)).map_err(storage_error)?);
        let receipt = format!("{}requests/{request_id}", self.prefix);
        if let Some(raw) = self.db.get(cf::EXECUTION_STATE, receipt.as_bytes()).map_err(storage_error)? {
            let (previous, record): (String, StoredRecord) = serde_json::from_slice(&raw).map_err(storage_error)?;
            if previous != fingerprint { return Err(fault("request_conflict", "Request ID was already used for different content")); }
            return Ok(record);
        }
        let current = self.get(id)?;
        let revision = current.as_ref().map_or(0, |record| record.revision);
        if revision != expected {
            return Err(ResourceError { code: "revision_conflict".into(), message: "Resource changed; reload before editing".into(), expected_revision: Some(expected), actual_revision: Some(revision), current_digest: current.map(|r| r.content_digest) });
        }
        let record = StoredRecord { schema_version: SCHEMA_VERSION, revision: revision.checked_add(1).ok_or_else(|| fault("revision_exhausted", "Revision limit reached"))?, content_digest: digest(&serde_json::to_vec(&value).map_err(storage_error)?), value };
        let raw = serde_json::to_vec(&record).map_err(storage_error)?;
        self.db.resource_batch(&[
            (self.key(id).into_bytes(), raw.clone()),
            (format!("{}history/{id}/{:020}", self.prefix, record.revision).into_bytes(), raw),
            (receipt.into_bytes(), serde_json::to_vec(&(fingerprint, &record)).map_err(storage_error)?),
        ]).map_err(storage_error)?;
        Ok(record)
    }
}
