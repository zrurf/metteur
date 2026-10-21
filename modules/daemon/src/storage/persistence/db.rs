//! RocksDB persistence wrapper.

use std::path::Path;
use std::sync::Arc;

use rocksdb::{ColumnFamilyDescriptor, DB};

use crate::error::{DaemonError, DaemonResult};

/// Column family names used by the daemon.
pub mod cf {
    /// Stores blueprints keyed by blueprint id.
    pub const BLUEPRINTS: &str = "blueprints";
    /// Stores version snapshot metadata.
    pub const SNAPSHOTS: &str = "snapshots";
    /// Stores file content blobs keyed by content hash.
    pub const FILE_BLOBS: &str = "file_blobs";
    /// Stores audit log entries.
    pub const AUDIT_LOG: &str = "audit_log";
    /// Stores execution checkpoints keyed by run id.
    pub const EXECUTION_STATE: &str = "execution_state";
    /// Durable file-operation intents; content lives in the shared blob store.
    pub const FILE_INTENTS: &str = "file_intents";
    /// Stores persistent sandbox grants keyed by command hash.
    pub const GRANTS: &str = "grants";
    /// Stores blueprint functions keyed by function name.
    pub const FUNCTIONS: &str = "functions";
    /// Stores the workspace's active chat session.
    pub const CHAT_SESSIONS: &str = "chat_sessions";
    /// Stores chat threads keyed by session id.
    pub const CHAT_THREADS: &str = "chat_threads";
    /// Complete pre-turn chat state keyed by its file snapshot id.
    pub const CHAT_CHECKPOINTS: &str = "chat_checkpoints";
}

/// A thin wrapper around a RocksDB instance with typed column families.
#[derive(Clone)]
pub struct Db {
    inner: Arc<DB>,
    pub(crate) resource_gate: Arc<std::sync::Mutex<()>>,
    pub(crate) prompt_gate: Arc<std::sync::Mutex<()>>,
    /// Serializes independent oversight records across all clones of this database.
    pub(crate) oversight_gate: Arc<std::sync::Mutex<()>>,
    pub(crate) oversight_notify: Arc<tokio::sync::Notify>,
}

impl Db {
    pub fn directory(&self) -> &Path { self.inner.path() }
    /// Publish a derived graph and its file identity together in the existing family.
    pub fn put_pair(&self, family: &str, key: &[u8], value: &[u8], second_key: &[u8], second_value: &[u8]) -> DaemonResult<()> {
        let handle = self.inner.cf_handle(family).ok_or_else(|| DaemonError::Internal("missing column family".into()))?;
        let mut batch = rocksdb::WriteBatch::default();
        batch.put_cf(handle, key, value);
        batch.put_cf(handle, second_key, second_value);
        self.inner.write(batch).map_err(|e| DaemonError::Persistence(e.to_string()))
    }
    /// Commits review ownership and request lineage together before acknowledgment.
    pub fn put_pair_durable(&self, family: &str, key: &[u8], value: &[u8], second_key: &[u8], second_value: &[u8]) -> DaemonResult<()> {
        let handle = self.inner.cf_handle(family).ok_or_else(|| DaemonError::Persistence("missing column family".into()))?;
        let mut batch = rocksdb::WriteBatch::default();
        batch.put_cf(handle, key, value);
        batch.put_cf(handle, second_key, second_value);
        let mut options = rocksdb::WriteOptions::default(); options.set_sync(true);
        self.inner.write_opt(batch, &options).map_err(|e| DaemonError::Persistence(e.to_string()))
    }
    pub fn chat_revision(&self, session: &[u8]) -> DaemonResult<u64> {
        let key = [b"nx/v1/chat-revision/".as_slice(), session].concat();
        self.get(cf::EXECUTION_STATE, &key)?.map(|raw| serde_json::from_slice(&raw).map_err(|e| DaemonError::Persistence(e.to_string()))).transpose().map(|v| v.unwrap_or(0))
    }

    /// The caller holds resource_gate. Session revisions are outside rewind data.
    pub(crate) fn continuation_batch(&self, values: &[(Vec<u8>, Vec<u8>)], session: Option<(&[u8], &[u8], u64)>) -> DaemonResult<()> {
        let resources = self.inner.cf_handle(cf::EXECUTION_STATE).ok_or_else(|| DaemonError::Persistence("Missing resource family".into()))?;
        let mut batch = rocksdb::WriteBatch::default();
        for (key, value) in values { batch.put_cf(resources, key, value); }
        if let Some((id, data, expected)) = session {
            let current = self.chat_revision(id)?;
            if current != expected { return Err(DaemonError::Execution("Session revision changed".into())); }
            let revision = current.checked_add(1).ok_or_else(|| DaemonError::Persistence("Session revision exhausted".into()))?;
            let threads = self.inner.cf_handle(cf::CHAT_THREADS).ok_or_else(|| DaemonError::Persistence("Missing chat family".into()))?;
            batch.put_cf(threads, id, data);
            batch.put_cf(resources, [b"nx/v1/chat-revision/".as_slice(), id].concat(), serde_json::to_vec(&revision).map_err(|e| DaemonError::Persistence(e.to_string()))?);
        }
        let mut options = rocksdb::WriteOptions::default(); options.set_sync(true);
        self.inner.write_opt(batch, &options).map_err(|e| DaemonError::Persistence(e.to_string()))
    }

    pub fn save_chat_revision(&self, id: &[u8], data: &[u8]) -> DaemonResult<()> {
        let _guard = self.resource_gate.lock().map_err(|_| DaemonError::Persistence("Session revision lock unavailable".into()))?;
        self.continuation_batch(&[], Some((id, data, self.chat_revision(id)?)))
    }

    /// Atomically replaces a chat thread and removes invalidated checkpoints.
    pub fn commit_chat_rewind(
        &self,
        session_id: &[u8],
        data: &[u8],
        obsolete: &[Vec<u8>],
    ) -> DaemonResult<()> {
        let _guard = self.resource_gate.lock().map_err(|_| DaemonError::Persistence("Session revision lock unavailable".into()))?;
        let revision = self.chat_revision(session_id)?.checked_add(1).ok_or_else(|| DaemonError::Persistence("Session revision exhausted".into()))?;
        let resources = self.inner.cf_handle(cf::EXECUTION_STATE).ok_or_else(|| DaemonError::Persistence("Missing resource family".into()))?;
        let threads = self
            .inner
            .cf_handle(cf::CHAT_THREADS)
            .ok_or_else(|| DaemonError::Internal("missing chat threads family".into()))?;
        let checkpoints = self
            .inner
            .cf_handle(cf::CHAT_CHECKPOINTS)
            .ok_or_else(|| DaemonError::Internal("missing chat checkpoints family".into()))?;
        let mut batch = rocksdb::WriteBatch::default();
        batch.put_cf(threads, session_id, data);
        batch.put_cf(resources, [b"nx/v1/chat-revision/".as_slice(), session_id].concat(), serde_json::to_vec(&revision).map_err(|e| DaemonError::Persistence(e.to_string()))?);
        for key in obsolete {
            batch.delete_cf(checkpoints, key);
        }
        self.inner
            .write(batch)
            .map_err(|e| DaemonError::Internal(format!("chat rewind commit failed: {e}")))
    }

    /// Opens (or creates) a database at `path` with the standard column
    /// families.
    pub fn open(path: &Path) -> DaemonResult<Self> {
        std::fs::create_dir_all(path)?;
        let mut opts = rocksdb::Options::default();
        opts.create_if_missing(true);
        opts.create_missing_column_families(true);

        let cfs = [
            ColumnFamilyDescriptor::new(cf::BLUEPRINTS, rocksdb::Options::default()),
            ColumnFamilyDescriptor::new(cf::SNAPSHOTS, rocksdb::Options::default()),
            ColumnFamilyDescriptor::new(cf::FILE_BLOBS, rocksdb::Options::default()),
            ColumnFamilyDescriptor::new(cf::AUDIT_LOG, rocksdb::Options::default()),
            ColumnFamilyDescriptor::new(cf::EXECUTION_STATE, rocksdb::Options::default()),
            ColumnFamilyDescriptor::new(cf::FILE_INTENTS, rocksdb::Options::default()),
            ColumnFamilyDescriptor::new(cf::GRANTS, rocksdb::Options::default()),
            ColumnFamilyDescriptor::new(cf::FUNCTIONS, rocksdb::Options::default()),
            ColumnFamilyDescriptor::new(cf::CHAT_SESSIONS, rocksdb::Options::default()),
            ColumnFamilyDescriptor::new(cf::CHAT_THREADS, rocksdb::Options::default()),
            ColumnFamilyDescriptor::new(cf::CHAT_CHECKPOINTS, rocksdb::Options::default()),
        ];

        // RocksDB on Windows rejects the `\\?\` extended-length path prefix
        // that `canonicalize` may produce, so normalize it away.
        let db_path = normalize_path(path);
        let db = DB::open_cf_descriptors(&opts, &db_path, cfs)
            .map_err(|e| DaemonError::Internal(format!("failed to open db: {e}")))?;
        Ok(Self {
            inner: Arc::new(db),
            resource_gate: Arc::new(std::sync::Mutex::new(())),
            prompt_gate: Arc::new(std::sync::Mutex::new(())),
            oversight_gate: Arc::new(std::sync::Mutex::new(())),
            oversight_notify: Arc::new(tokio::sync::Notify::new()),
        })
    }

    /// Creates a consistent checkpoint without copying a live database directory.
    pub fn checkpoint(&self, path: &Path) -> DaemonResult<()> {
        // As with opening a database, RocksDB needs the ordinary Windows path:
        // its checkpoint implementation appends '/' before linking each file.
        let path = normalize_path(path);
        rocksdb::checkpoint::Checkpoint::new(&self.inner)
            .and_then(|checkpoint| checkpoint.create_checkpoint(path))
            .map_err(|e| DaemonError::Persistence(e.to_string()))
    }

    /// Commits a small resource transaction with synchronous WAL durability.
    pub(crate) fn resource_batch(&self, values: &[(Vec<u8>, Vec<u8>)]) -> DaemonResult<()> {
        let handle = self.inner.cf_handle(cf::EXECUTION_STATE)
            .ok_or_else(|| DaemonError::Persistence("missing resource family".into()))?;
        let mut batch = rocksdb::WriteBatch::default();
        for (key, value) in values { batch.put_cf(handle, key, value); }
        let mut options = rocksdb::WriteOptions::default();
        options.set_sync(true);
        self.inner.write_opt(batch, &options).map_err(|e| DaemonError::Persistence(e.to_string()))
    }

    pub(crate) fn resource_scan(&self, prefix: &[u8], after: &[u8], limit: usize) -> DaemonResult<Vec<(Vec<u8>, Vec<u8>)>> {
        let handle = self.inner.cf_handle(cf::EXECUTION_STATE)
            .ok_or_else(|| DaemonError::Persistence("missing resource family".into()))?;
        let start = if after.is_empty() { prefix } else { after };
        let mut out = Vec::new();
        for item in self.inner.iterator_cf(handle, rocksdb::IteratorMode::From(start, rocksdb::Direction::Forward)) {
            let (key, value) = item.map_err(|e| DaemonError::Persistence(e.to_string()))?;
            if !key.starts_with(prefix) { break; }
            if !after.is_empty() && key.as_ref() <= after { continue; }
            out.push((key.to_vec(), value.to_vec()));
            if out.len() >= limit { break; }
        }
        Ok(out)
    }

    /// Puts a value into a column family.
    pub fn put(&self, cf: &str, key: &[u8], value: &[u8]) -> DaemonResult<()> {
        let handle = self
            .inner
            .cf_handle(cf)
            .ok_or_else(|| DaemonError::Internal(format!("unknown column family {cf}")))?;
        self.inner
            .put_cf(handle, key, value)
            .map_err(|e| DaemonError::Internal(format!("db put failed: {e}")))?;
        Ok(())
    }

    /// Acknowledged side-channel state must survive a process or machine crash.
    pub fn put_durable(&self, family: &str, key: &[u8], value: &[u8]) -> DaemonResult<()> {
        let handle = self.inner.cf_handle(family).ok_or_else(|| DaemonError::Persistence("missing column family".into()))?;
        let mut options = rocksdb::WriteOptions::default();
        options.set_sync(true);
        self.inner.put_cf_opt(handle, key, value, &options).map_err(|e| DaemonError::Persistence(e.to_string()))
    }

    /// Gets a value from a column family.
    pub fn get(&self, cf: &str, key: &[u8]) -> DaemonResult<Option<Vec<u8>>> {
        let handle = self
            .inner
            .cf_handle(cf)
            .ok_or_else(|| DaemonError::Internal(format!("unknown column family {cf}")))?;
        self.inner
            .get_cf(handle, key)
            .map_err(|e| DaemonError::Internal(format!("db get failed: {e}")))
    }

    /// Deletes a key from a column family.
    ///
    /// Deleting is idempotent: removing a missing key succeeds silently.
    pub fn delete(&self, cf: &str, key: &[u8]) -> DaemonResult<()> {
        let handle = self
            .inner
            .cf_handle(cf)
            .ok_or_else(|| DaemonError::Internal(format!("unknown column family {cf}")))?;
        self.inner
            .delete_cf(handle, key)
            .map_err(|e| DaemonError::Internal(format!("db delete failed: {e}")))?;
        Ok(())
    }

    /// Iterates over all key-value pairs in a column family.
    pub fn scan(&self, cf: &str) -> DaemonResult<Vec<(Vec<u8>, Vec<u8>)>> {
        let handle = self
            .inner
            .cf_handle(cf)
            .ok_or_else(|| DaemonError::Internal(format!("unknown column family {cf}")))?;
        let iter = self.inner.iterator_cf(handle, rocksdb::IteratorMode::Start);
        let mut out = Vec::new();
        for item in iter {
            let (k, v) = item.map_err(|e| DaemonError::Internal(format!("db scan failed: {e}")))?;
            out.push((k.to_vec(), v.to_vec()));
        }
        Ok(out)
    }
}

/// Converts a path to a string usable by RocksDB.
///
/// On Windows, strips the `\\?\` extended-length prefix that `canonicalize`
/// may add, which RocksDB does not accept.
fn normalize_path(path: &Path) -> String {
    let s = path.to_string_lossy();
    #[cfg(windows)]
    {
        s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
    }
    #[cfg(not(windows))]
    {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_get_roundtrip() {
        let dir = std::env::temp_dir().join(format!("metteur-db-{}", uuid::Uuid::new_v4()));
        let db = Db::open(&dir).unwrap();
        db.put(cf::BLUEPRINTS, b"key1", b"value1").unwrap();
        assert_eq!(db.get(cf::BLUEPRINTS, b"key1").unwrap().unwrap(), b"value1");
    }

    #[test]
    fn canonical_checkpoint_preserves_live_data() {
        let root = std::env::temp_dir().join(format!("metteur-db-checkpoint-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let db = Db::open(&root.join("source")).unwrap();
        db.put_durable(cf::BLUEPRINTS, b"key", b"before").unwrap();
        let checkpoint = root.canonicalize().unwrap().join("checkpoint");
        db.checkpoint(&checkpoint).unwrap();
        db.put_durable(cf::BLUEPRINTS, b"key", b"after").unwrap();
        let restored = Db::open(&checkpoint).unwrap();
        assert_eq!(restored.get(cf::BLUEPRINTS, b"key").unwrap().unwrap(), b"before");
        assert_eq!(db.get(cf::BLUEPRINTS, b"key").unwrap().unwrap(), b"after");
    }
}
