//! File references use existing snapshots/blobs, never a separate version chain.
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{VersionManager, hash_content, restore};
use crate::error::{DaemonError, DaemonResult};
use crate::execution::file_journal::{
    DbFileJournal, FileJournal, FileOrigin, read_optional, validate_path,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionRef {
    pub snapshot_id: Uuid,
    pub blueprint_uri: String,
    pub blob_hash: String,
}

#[derive(Clone, Copy)]
pub(crate) enum ExpectedFile<'a> {
    Any,
    /// Manual save may update only a valid file with the same identity.
    Blueprint(Uuid),
    Absent,
    Version(&'a VersionRef),
}
impl<'a> From<Option<&'a VersionRef>> for ExpectedFile<'a> {
    fn from(value: Option<&'a VersionRef>) -> Self {
        value.map(Self::Version).unwrap_or(Self::Any)
    }
}

impl VersionManager {
    pub fn blueprint_path(&self, uri: &str) -> DaemonResult<std::path::PathBuf> {
        let relative = std::path::PathBuf::from(uri.replace('\\', "/"));
        let path = if relative.is_absolute() {
            relative
        } else {
            self.root.join(relative)
        };
        validate_path(&self.root, &path)?;
        let rel = path
            .strip_prefix(&self.root)
            .map_err(|_| DaemonError::PermissionDenied("blueprint outside workspace".into()))?;
        if restore::excluded(rel) {
            return Err(DaemonError::Execution(
                "choose a blueprint file tracked by Version Flow (not an excluded directory)"
                    .into(),
            ));
        }
        Ok(path)
    }

    /// Force a content read: mtime/size alone cannot bind an approved proposal.
    pub fn capture_blueprint(&self, uri: &str) -> DaemonResult<VersionRef> {
        let path = self.blueprint_path(uri)?;
        let _guard = self.operation_gate.lock();
        self.capture_blueprint_locked(&path)
    }

    fn capture_blueprint_locked(&self, path: &Path) -> DaemonResult<VersionRef> {
        let rel = path
            .strip_prefix(&self.root)
            .map_err(|_| DaemonError::PermissionDenied("blueprint outside workspace".into()))?
            .to_string_lossy()
            .replace('\\', "/");
        let (snapshot, changed) = self.build_snapshot_for("Blueprint file", None, Some(&rel))?;
        let snapshot = if changed {
            self.store_snapshot(&snapshot)?;
            snapshot
        } else {
            self.latest_snapshot()?.unwrap_or(snapshot)
        };
        let blob_hash = snapshot
            .files
            .get(&rel)
            .cloned()
            .ok_or_else(|| DaemonError::NotFound(format!("blueprint file {rel}")))?;
        Ok(VersionRef {
            snapshot_id: snapshot.id,
            blueprint_uri: rel,
            blob_hash,
        })
    }

    pub fn verify_blueprint(&self, version: &VersionRef) -> DaemonResult<()> {
        let path = self.blueprint_path(&version.blueprint_uri)?;
        let bytes = std::fs::read(path)?;
        if hash_content(&bytes) != version.blob_hash {
            return Err(DaemonError::Execution(
                "blueprint file changed; save or propose again".into(),
            ));
        }
        Ok(())
    }

    /// Keep watcher snapshots outside the before/intent/write/after interval.
    pub fn write_blueprint_file(
        &self,
        uri: &str,
        bytes: &[u8],
        expected: Option<&VersionRef>,
    ) -> DaemonResult<VersionRef> {
        self.write_blueprint_with_origin(
            uri,
            bytes,
            expected.into(),
            FileOrigin {
                run_id: Uuid::nil(),
                node_id: Uuid::nil(),
                attempt: 0,
                wal_position: 0,
            },
        )
        .map(|(version, _)| version)
    }

    pub(crate) fn write_blueprint_with_origin(
        &self,
        uri: &str,
        bytes: &[u8],
        expected: ExpectedFile<'_>,
        origin: FileOrigin,
    ) -> DaemonResult<(VersionRef, Option<Uuid>)> {
        let path = self.blueprint_path(uri)?;
        let _guard = self.operation_gate.lock();
        if let ExpectedFile::Version(base) = expected {
            if self.blueprint_path(&base.blueprint_uri)? != path {
                return Err(DaemonError::Execution(
                    "blueprint path does not match proposal".into(),
                ));
            }
            self.verify_blueprint(base)?;
        }
        let mut operation_id = None;
        let before = read_optional(&path)?;
        if let ExpectedFile::Blueprint(id) = expected
            && let Some(previous) = &before
        {
            let graph = crate::storage::blueprint_files::decode(previous).map_err(|_| {
                DaemonError::Execution("existing blueprint file is invalid; save to a new file".into())
            })?;
            if graph.id != id {
                return Err(DaemonError::Execution(
                    "existing file contains a different blueprint id; choose another file or its explicit id".into(),
                ));
            }
            if previous != bytes && std::fs::metadata(&path)?.permissions().readonly() {
                return Err(DaemonError::PermissionDenied("blueprint file is read-only".into()));
            }
        }
        let mismatch = match expected {
            ExpectedFile::Any | ExpectedFile::Blueprint(_) => false,
            ExpectedFile::Absent => before.is_some(),
            ExpectedFile::Version(base) => {
                before.as_deref().map(hash_content).as_ref() != Some(&base.blob_hash)
            }
        };
        if mismatch {
            return Err(DaemonError::Execution("blueprint changed before preparation".into()));
        }
        if before.as_deref() != Some(bytes) {
            // Even first-save absence must have an explicit pre-change snapshot.
            let rel = path.strip_prefix(&self.root).unwrap().to_string_lossy().replace('\\', "/");
            let (snapshot, changed) =
                self.build_snapshot_for("Before blueprint save", None, Some(&rel))?;
            if changed {
                self.store_snapshot(&snapshot)?;
            }
            let journal =
                FileJournal::new(self.root.clone(), Arc::new(DbFileJournal(self.db.clone())));
            let mut operation = journal.prepare(&path, Some(bytes), origin, before.as_deref())?;
            if operation.before != before.as_deref().map(hash_content) {
                operation.phase = crate::execution::file_journal::FilePhase::Conflict;
                journal.save(&operation)?;
                return Err(DaemonError::Persistence(
                    "blueprint appeared during save; recovery required".into(),
                ));
            }
            journal.apply(&mut operation)?;
            operation_id = Some(operation.operation_id);
        }
        Ok((self.capture_blueprint_locked(&path)?, operation_id))
    }
}
