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
    /// Serializes independent oversight records across all clones of this database.
    pub(crate) oversight_gate: Arc<std::sync::Mutex<()>>,
    pub(crate) oversight_notify: Arc<tokio::sync::Notify>,
}

impl Db {
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
    /// Atomically replaces a chat thread and removes invalidated checkpoints.
    pub fn commit_chat_rewind(
        &self,
        session_id: &[u8],
        data: &[u8],
        obsolete: &[Vec<u8>],
    ) -> DaemonResult<()> {
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
            oversight_gate: Arc::new(std::sync::Mutex::new(())),
            oversight_notify: Arc::new(tokio::sync::Notify::new()),
        })
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
}
