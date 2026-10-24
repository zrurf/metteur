//! Durable intents for regular files. Shell effects, directories, links and
//! file attributes are outside this content-recovery protocol.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use crate::error::{DaemonError, DaemonResult};
use crate::storage::persistence::{Db, cf};
use crate::storage::versioning::hash_content;
use uuid::Uuid;

/// The durable phase of a single file operation.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FilePhase {
    Prepared,
    #[default]
    Applied,
    Reverting,
    Reverted,
    Conflict,
}

/// Origin and order within the run's shared transaction log.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FileOrigin {
    pub run_id: Uuid,
    pub node_id: Uuid,
    pub attempt: u32,
    pub wal_position: usize,
}

/// Content references use the existing Version Flow blob keys. `None` means
/// absent; an empty file has a real blob and is distinct from absence.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FileOperation {
    pub operation_id: Uuid,
    pub origin: FileOrigin,
    pub path: PathBuf,
    pub before: Option<String>,
    pub after: Option<String>,
    pub phase: FilePhase,
}

/// Minimal storage seam for durable writes and boundary-failure tests.
pub trait FileJournalStore: Send + Sync + std::fmt::Debug {
    fn save(&self, operation: &FileOperation) -> DaemonResult<()>;
    fn operations(&self) -> DaemonResult<Vec<FileOperation>>;
    fn put_blob(&self, bytes: &[u8]) -> DaemonResult<String>;
    fn blob(&self, hash: &str) -> DaemonResult<Vec<u8>>;
}

/// The workspace's existing RocksDB and content-addressed blob family.
#[derive(Clone)]
pub struct DbFileJournal(pub Db);

impl std::fmt::Debug for DbFileJournal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DbFileJournal")
    }
}

impl FileJournalStore for DbFileJournal {
    fn save(&self, operation: &FileOperation) -> DaemonResult<()> {
        let bytes =
            serde_json::to_vec(operation).map_err(|e| DaemonError::Serialization(e.to_string()))?;
        self.0.put(cf::FILE_INTENTS, operation.operation_id.as_bytes(), &bytes)
    }
    fn operations(&self) -> DaemonResult<Vec<FileOperation>> {
        self.0
            .scan(cf::FILE_INTENTS)?
            .into_iter()
            .map(|(_, bytes)| {
                serde_json::from_slice(&bytes)
                    .map_err(|e| DaemonError::Serialization(e.to_string()))
            })
            .collect()
    }
    fn put_blob(&self, bytes: &[u8]) -> DaemonResult<String> {
        let hash = hash_content(bytes);
        if let Some(existing) = self.0.get(cf::FILE_BLOBS, hash.as_bytes())? {
            if existing != bytes {
                return Err(DaemonError::Persistence("file blob hash collision".into()));
            }
        } else {
            self.0.put(cf::FILE_BLOBS, hash.as_bytes(), bytes)?;
        }
        Ok(hash)
    }
    fn blob(&self, hash: &str) -> DaemonResult<Vec<u8>> {
        self.0
            .get(cf::FILE_BLOBS, hash.as_bytes())?
            .ok_or_else(|| DaemonError::Persistence(format!("missing file recovery blob {hash}")))
    }
}

/// One shared journal per run. Records/blobs are retained while recovery or
/// rollback may reference them; this module intentionally performs no GC.
#[derive(Debug, Clone)]
pub struct FileJournal {
    pub root: PathBuf,
    pub store: Arc<dyn FileJournalStore>,
}

impl FileJournal {
    pub fn new(root: PathBuf, store: Arc<dyn FileJournalStore>) -> Self {
        Self {
            root,
            store,
        }
    }

    /// Persist content and intent before the filesystem can be changed.
    pub fn prepare(
        &self,
        path: &Path,
        after: Option<&[u8]>,
        origin: FileOrigin,
        expected: Option<&[u8]>,
    ) -> DaemonResult<FileOperation> {
        self.prepare_checked(path, after, origin, expected, false)
    }
    pub fn prepare_new(
        &self,
        path: &Path,
        after: &[u8],
        origin: FileOrigin,
    ) -> DaemonResult<FileOperation> {
        self.prepare_checked(path, Some(after), origin, None, true)
    }
    fn prepare_checked(
        &self,
        path: &Path,
        after: Option<&[u8]>,
        origin: FileOrigin,
        expected: Option<&[u8]>,
        create_only: bool,
    ) -> DaemonResult<FileOperation> {
        validate_path(&self.root, path)?;
        let before = read_optional(path)?;
        if create_only && before.is_some() {
            return Err(DaemonError::Execution(
                "file already exists; choose a new copy path".into(),
            ));
        }
        if expected.is_some_and(|value| before.as_deref() != Some(value)) {
            return Err(DaemonError::Execution(format!(
                "file changed before edit: {}",
                path.display()
            )));
        }
        if self.store.operations()?.iter().any(|op| {
            op.path == path
                && matches!(
                    op.phase,
                    FilePhase::Prepared | FilePhase::Reverting | FilePhase::Conflict
                )
        }) {
            return Err(DaemonError::Persistence(format!(
                "unresolved file operation for {}; manual recovery required",
                path.display()
            )));
        }
        let before = before.as_deref().map(|bytes| self.store.put_blob(bytes)).transpose()?;
        let after = after.map(|bytes| self.store.put_blob(bytes)).transpose()?;
        let operation = FileOperation {
            operation_id: Uuid::new_v4(),
            origin,
            path: path.to_path_buf(),
            before,
            after,
            phase: FilePhase::Prepared,
        };
        self.save(&operation)?;
        Ok(operation)
    }

    /// Apply only a prepared intent, then persist its outcome.
    pub fn apply(&self, operation: &mut FileOperation) -> DaemonResult<()> {
        validate_path(&self.root, &operation.path)?;
        if operation.phase != FilePhase::Prepared
            || !self.matches(&operation.path, &operation.before)?
        {
            return Err(DaemonError::Persistence(format!(
                "file changed after preparation: {}; recovery required",
                operation.path.display()
            )));
        }
        if operation.before.is_none()
            && let Some(hash) = &operation.after
        {
            use std::io::Write;
            if let Some(parent) = operation.path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let bytes = self.store.blob(hash)?;
            let mut file =
                std::fs::OpenOptions::new().write(true).create_new(true).open(&operation.path)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
        } else {
            self.restore(&operation.path, &operation.after)?;
        }
        operation.phase = FilePhase::Applied;
        self.save(operation)
    }

    pub fn save(&self, operation: &FileOperation) -> DaemonResult<()> {
        self.store.save(operation).map_err(|e| {
            DaemonError::Persistence(format!(
                "file operation {} ({:?}) could not be saved: {e}",
                operation.operation_id, operation.phase
            ))
        })
    }

    pub fn matches(&self, path: &Path, image: &Option<String>) -> DaemonResult<bool> {
        let expected = image.as_deref().map(|hash| self.store.blob(hash)).transpose()?;
        Ok(read_optional(path)? == expected)
    }

    pub(crate) fn restore(&self, path: &Path, image: &Option<String>) -> DaemonResult<()> {
        validate_path(&self.root, path)?;
        match image {
            Some(hash) => {
                let bytes = self.store.blob(hash)?;
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(path, bytes)?;
            }
            None => match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            },
        }
        Ok(())
    }
}

pub(crate) fn read_optional(path: &Path) -> DaemonResult<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Reject links in every component, unsupported file types and metadata paths.
/// This is not a defense against a hostile concurrent filesystem namespace swap.
pub fn validate_path(root: &Path, path: &Path) -> DaemonResult<()> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| DaemonError::PermissionDenied("file path outside workspace".into()))?;
    if relative.as_os_str().is_empty()
        || relative.starts_with(crate::workspace::METADATA_DIR)
        || relative.components().any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(DaemonError::PermissionDenied("unsupported recovery path".into()));
    }
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        match std::fs::symlink_metadata(&current) {
            Ok(meta) => {
                let link = crate::workspace::fs::is_link(&meta);
                #[cfg(unix)]
                let link = {
                    use std::os::unix::fs::MetadataExt;
                    link || meta.is_file() && meta.nlink() > 1
                };
                if link
                    || (current == path && !meta.is_file())
                    || (current != path && !meta.is_dir())
                {
                    return Err(DaemonError::PermissionDenied(format!(
                        "file recovery does not support links or special files: {}",
                        current.display()
                    )));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
