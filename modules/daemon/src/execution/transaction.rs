//! Write-ahead transaction log for execution rollback.

use std::path::PathBuf;

use super::file_journal::{FileJournal, FileOrigin, FilePhase, read_optional, validate_path};
use crate::error::{DaemonError, DaemonResult};
use metteur_shared::Value;

/// A single recorded mutation during execution.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum TransactionEntry {
    /// A file was written (or created).
    FileWrite {
        /// The file path.
        path: PathBuf,
        /// The previous content, if the file already existed.
        old_content: Option<Vec<u8>>,
        /// The new content.
        new_content: Vec<u8>,
        #[serde(default)]
        reverted: bool,
    },
    /// A file was deleted.
    FileDelete {
        /// The file path.
        path: PathBuf,
        /// The content before deletion.
        content: Vec<u8>,
        #[serde(default)]
        reverted: bool,
    },
    /// A durable operation; before/after content is in the shared blob store.
    FileOperation {
        operation_id: uuid::Uuid,
        #[serde(default)]
        phase: FilePhase,
    },
    /// A user-approved plan change, retained across validation/cancel rollback.
    /// Its file operation still belongs to this checkpoint for resume checks.
    BlueprintApplication {
        operation_id: uuid::Uuid,
    },
    /// A tool was invoked.
    ToolCall {
        /// The tool name.
        name: String,
        /// The tool arguments.
        args: Vec<Value>,
    },
}

/// A write-ahead log of mutations performed during an execution.
///
/// Cloning shares the same underlying entries, so nested runs (SubAgent /
/// abstract blueprints) record into the parent log and a rollback of the
/// outer run undoes their file mutations as well.
#[derive(Debug, Default, Clone)]
pub struct TransactionLog {
    entries: std::sync::Arc<std::sync::Mutex<Vec<TransactionEntry>>>,
    journal: std::sync::Arc<std::sync::Mutex<Option<FileJournal>>>,
    failure: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    owner: std::sync::Arc<std::sync::Mutex<Option<uuid::Uuid>>>,
}

impl TransactionLog {
    /// Creates an empty transaction log.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a file write.
    pub fn record_file_write(
        &self,
        path: PathBuf,
        old_content: Option<Vec<u8>>,
        new_content: Vec<u8>,
    ) {
        self.lock().push(TransactionEntry::FileWrite {
            path,
            old_content,
            new_content,
            reverted: false,
        });
    }

    /// Records a file delete.
    pub fn record_file_delete(&self, path: PathBuf, content: Vec<u8>) {
        self.lock().push(TransactionEntry::FileDelete {
            path,
            content,
            reverted: false,
        });
    }

    /// Records a tool call.
    pub fn record_tool_call(&self, name: String, args: Vec<Value>) {
        self.lock().push(TransactionEntry::ToolCall {
            name,
            args,
        });
    }

    pub(crate) fn record_blueprint_application(&self, operation_id: uuid::Uuid) {
        self.lock().push(TransactionEntry::BlueprintApplication {
            operation_id,
        });
    }

    /// Returns a snapshot of the recorded entries.
    pub fn entries(&self) -> Vec<TransactionEntry> {
        self.lock().clone()
    }

    /// Creates a log from a previously recorded entry list.
    pub fn from_entries(entries: Vec<TransactionEntry>) -> Self {
        Self {
            entries: std::sync::Arc::new(std::sync::Mutex::new(entries)),
            ..Self::default()
        }
    }

    /// Attaches the workspace journal once; clones share it with nested runs.
    pub fn with_journal(self, journal: FileJournal) -> Self {
        self.attach_journal(journal);
        self
    }

    pub fn attach_journal(&self, journal: FileJournal) {
        let mut current = self.journal.lock().unwrap();
        if current.is_none() {
            *current = Some(journal);
        }
    }

    /// Nested interpreters without their own checkpoint retain the root run.
    pub fn bind_run(&self, run_id: uuid::Uuid) {
        if !run_id.is_nil() {
            self.owner.lock().unwrap().get_or_insert(run_id);
        }
    }

    /// A failed durable write poisons the shared run, including nested tools.
    pub fn ensure_healthy(&self) -> DaemonResult<()> {
        if let Some(error) = self.failure.lock().unwrap().as_ref() {
            return Err(DaemonError::Persistence(error.clone()));
        }
        Ok(())
    }

    /// Writes/deletes a regular file. Production workspace contexts attach a
    /// durable journal; standalone ephemeral contexts retain in-memory rollback.
    pub fn mutate_file(
        &self,
        root: &std::path::Path,
        path: &std::path::Path,
        after: Option<&[u8]>,
        origin: FileOrigin,
        expected: Option<&[u8]>,
    ) -> DaemonResult<()> {
        self.mutate_file_checked(root, path, after, origin, expected, false)
    }
    pub fn create_file(
        &self,
        root: &std::path::Path,
        path: &std::path::Path,
        bytes: &[u8],
        origin: FileOrigin,
    ) -> DaemonResult<()> {
        self.mutate_file_checked(root, path, Some(bytes), origin, None, true)
    }
    fn mutate_file_checked(
        &self,
        root: &std::path::Path,
        path: &std::path::Path,
        after: Option<&[u8]>,
        mut origin: FileOrigin,
        expected: Option<&[u8]>,
        create_only: bool,
    ) -> DaemonResult<()> {
        self.ensure_healthy()?;
        validate_path(root, path)?;
        let mut entries = self.lock();
        origin.wal_position = entries.len();
        origin.run_id = self.owner.lock().unwrap().unwrap_or(origin.run_id);
        let journal = self.journal.lock().unwrap().clone();
        if let Some(journal) = journal {
            let result = (|| {
                let mut operation = if create_only {
                    journal.prepare_new(
                        path,
                        after.ok_or_else(|| {
                            DaemonError::Execution("new file needs content".into())
                        })?,
                        origin,
                    )?
                } else {
                    journal.prepare(path, after, origin, expected)?
                };
                entries.push(TransactionEntry::FileOperation {
                    operation_id: operation.operation_id,
                    phase: FilePhase::Prepared,
                });
                journal.apply(&mut operation)?;
                if let Some(TransactionEntry::FileOperation {
                    phase,
                    ..
                }) = entries.last_mut()
                {
                    *phase = FilePhase::Applied;
                }
                Ok::<_, DaemonError>(())
            })();
            if let Err(error) = result {
                let message = format!(
                    "file persistence/application failed for {}: {error}; recovery required",
                    path.display()
                );
                *self.failure.lock().unwrap() = Some(message.clone());
                return Err(DaemonError::Persistence(message));
            }
        } else {
            let before = read_optional(path)?;
            if create_only && before.is_some() {
                return Err(DaemonError::Execution(
                    "file already exists; choose a new copy path".into(),
                ));
            }
            if expected.is_some_and(|value| before.as_deref() != Some(value)) {
                return Err(DaemonError::Execution("file changed before edit".into()));
            }
            match after {
                Some(bytes) => {
                    entries.push(TransactionEntry::FileWrite {
                        path: path.into(),
                        old_content: before,
                        new_content: bytes.to_vec(),
                        reverted: false,
                    });
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    if create_only {
                        use std::io::Write;
                        let mut file =
                            std::fs::OpenOptions::new().write(true).create_new(true).open(path)?;
                        file.write_all(bytes)?;
                    } else {
                        std::fs::write(path, bytes)?;
                    }
                }
                None => {
                    if let Some(content) = before {
                        entries.push(TransactionEntry::FileDelete {
                            path: path.into(),
                            content,
                            reverted: false,
                        });
                        std::fs::remove_file(path)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<TransactionEntry>> {
        self.entries.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Reject checkpoints older than durable file mutations or undo decisions.
    pub fn validate_resume(&self, run_id: uuid::Uuid) -> DaemonResult<()> {
        let entries = self.lock();
        let journal = self.journal.lock().unwrap().clone();
        let durable: Vec<_> = entries
            .iter()
            .filter_map(|entry| match entry {
                TransactionEntry::FileOperation {
                    operation_id,
                    phase,
                } => Some((*operation_id, *phase)),
                TransactionEntry::BlueprintApplication {
                    operation_id,
                } => Some((*operation_id, FilePhase::Applied)),
                _ => None,
            })
            .collect();
        let Some(journal) = journal else {
            return if durable.is_empty() {
                Ok(())
            } else {
                Err(DaemonError::Persistence("file journal unavailable for resume".into()))
            };
        };
        let operations = journal.store.operations()?;
        for (id, phase) in &durable {
            if !operations.iter().any(|op| {
                op.operation_id == *id
                    && op.origin.run_id == run_id
                    && op.phase == *phase
                    && matches!(phase, FilePhase::Applied | FilePhase::Reverted)
            }) {
                return Err(DaemonError::Persistence(format!(
                    "checkpoint file operation {id} no longer matches durable state; manual recovery required"
                )));
            }
        }
        if operations.iter().any(|op| {
            op.origin.run_id == run_id && !durable.iter().any(|(id, _)| *id == op.operation_id)
        }) {
            return Err(DaemonError::Persistence(
                "file operations exist beyond this checkpoint; manual recovery required".into(),
            ));
        }
        Ok(())
    }

    /// Returns the current log length, usable as a rollback mark.
    ///
    /// A later [`Self::rollback_after`] with this value undoes exactly the
    /// mutations recorded after the mark was taken.
    pub fn mark(&self) -> usize {
        self.lock().len()
    }

    /// Reverses all recorded mutations, restoring the pre-execution state.
    ///
    /// File writes restore the previous content (or delete the file if it did
    /// not exist); file deletes restore the deleted file.
    pub fn rollback(&self) -> DaemonResult<()> {
        self.rollback_after(0).map(|_| ())
    }

    /// Reverses the mutations recorded after `mark`, returning their count.
    ///
    /// Entries are replayed in reverse order, so multiple writes to the same
    /// file converge on the earliest before-image. This also holds for the
    /// duplicated entries a re-executed node records after a crash/resume.
    /// Shell side effects (e.g. `ExecuteCommand`) are not captured by the WAL
    /// and therefore cannot be undone.
    pub fn rollback_after(&self, mark: usize) -> DaemonResult<usize> {
        let mut entries = self.lock();
        let mut undone = 0;
        let mut blocked = std::collections::HashSet::new();
        let mut conflicts = Vec::new();
        let journal = self.journal.lock().unwrap().clone();
        for entry in entries.iter_mut().skip(mark).rev() {
            let (path, result) = match entry {
                TransactionEntry::FileWrite {
                    path,
                    old_content,
                    new_content,
                    reverted,
                } => {
                    if *reverted || blocked.contains(path) {
                        continue;
                    }
                    let result = rollback_legacy(path, old_content.as_deref(), Some(new_content));
                    if result.is_ok() {
                        *reverted = true;
                    }
                    (path.clone(), result)
                }
                TransactionEntry::FileDelete {
                    path,
                    content,
                    reverted,
                } => {
                    if *reverted || blocked.contains(path) {
                        continue;
                    }
                    let result = rollback_legacy(path, Some(content), None);
                    if result.is_ok() {
                        *reverted = true;
                    }
                    (path.clone(), result)
                }
                TransactionEntry::FileOperation {
                    operation_id,
                    phase,
                } => {
                    let journal = journal.as_ref().ok_or_else(|| {
                        DaemonError::Persistence("file journal unavailable for rollback".into())
                    })?;
                    let mut operation = journal
                        .store
                        .operations()?
                        .into_iter()
                        .find(|op| op.operation_id == *operation_id)
                        .ok_or_else(|| {
                            DaemonError::Persistence("file operation missing for rollback".into())
                        })?;
                    if blocked.contains(&operation.path) {
                        continue;
                    }
                    let result = journal.rollback_operation(&mut operation);
                    if result.is_ok() {
                        *phase = operation.phase;
                    }
                    (operation.path, result)
                }
                TransactionEntry::ToolCall {
                    ..
                }
                | TransactionEntry::BlueprintApplication {
                    ..
                } => continue,
            };
            match result {
                Ok(changed) => undone += usize::from(changed),
                Err(error @ DaemonError::Persistence(_)) => {
                    return Err(DaemonError::Persistence(format!(
                        "rollback incomplete after {undone} restored operation(s): {error}"
                    )));
                }
                Err(error) => {
                    blocked.insert(path);
                    conflicts.push(error.to_string());
                }
            }
        }
        if conflicts.is_empty() {
            Ok(undone)
        } else {
            Err(DaemonError::Execution(format!(
                "rollback incomplete: restored {undone} operation(s); {}",
                conflicts.join("; ")
            )))
        }
    }
}

/// Older/ephemeral entries carry bytes inline but still require a content guard.
fn rollback_legacy(
    path: &std::path::Path,
    before: Option<&[u8]>,
    after: Option<&[u8]>,
) -> DaemonResult<bool> {
    if !path.is_absolute() {
        return Err(DaemonError::Execution(
            "legacy relative file path requires manual recovery".into(),
        ));
    }
    validate_path(path.ancestors().last().unwrap(), path)?;
    let current = read_optional(path)?;
    if current.as_deref() == before {
        return Ok(true);
    }
    if current.as_deref() != after {
        return Err(DaemonError::Execution(format!(
            "file conflict at {}; later content was preserved",
            path.display()
        )));
    }
    match before {
        Some(bytes) => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, bytes)?;
        }
        None => std::fs::remove_file(path)?,
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rollback_restores_overwritten_file() {
        let dir = std::env::temp_dir().join(format!("metteur-txn-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.txt");
        std::fs::write(&path, "v1").unwrap();

        let log = TransactionLog::new();
        log.record_file_write(path.clone(), Some(b"v1".to_vec()), b"v2".to_vec());
        std::fs::write(&path, "v2").unwrap();

        log.rollback().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "v1");
    }

    #[test]
    fn rollback_removes_created_file() {
        let dir = std::env::temp_dir().join(format!("metteur-txn-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("new.txt");

        let log = TransactionLog::new();
        log.record_file_write(path.clone(), None, b"data".to_vec());
        std::fs::write(&path, "data").unwrap();

        log.rollback().unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn rollback_after_undoes_only_entries_past_the_mark() {
        let dir = std::env::temp_dir().join(format!("metteur-txn-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let kept = dir.join("kept.txt");
        let reverted = dir.join("reverted.txt");
        std::fs::write(&kept, "v1").unwrap();

        let log = TransactionLog::new();
        log.record_file_write(kept.clone(), Some(b"v1".to_vec()), b"v2".to_vec());
        std::fs::write(&kept, "v2").unwrap();

        let mark = log.mark();
        log.record_file_write(reverted.clone(), None, b"tmp".to_vec());
        std::fs::write(&reverted, "tmp").unwrap();

        let undone = log.rollback_after(mark).unwrap();
        assert_eq!(undone, 1);
        assert_eq!(std::fs::read_to_string(&kept).unwrap(), "v2");
        assert!(!reverted.exists());
    }

    #[test]
    fn rollback_after_zero_matches_full_rollback() {
        let dir = std::env::temp_dir().join(format!("metteur-txn-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.txt");
        std::fs::write(&path, "v1").unwrap();

        let log = TransactionLog::new();
        log.record_file_write(path.clone(), Some(b"v1".to_vec()), b"v2".to_vec());
        std::fs::write(&path, "v2").unwrap();

        log.rollback_after(0).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "v1");
    }

    #[test]
    fn repeated_writes_converge_on_the_earliest_before_image() {
        let dir = std::env::temp_dir().join(format!("metteur-txn-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.txt");
        std::fs::write(&path, "v1").unwrap();

        let log = TransactionLog::new();
        // Simulates a node executing twice (e.g. after a crash/resume) without
        // an intermediate rollback: both writes are recorded.
        log.record_file_write(path.clone(), Some(b"v1".to_vec()), b"v2".to_vec());
        log.record_file_write(path.clone(), Some(b"v2".to_vec()), b"v3".to_vec());
        std::fs::write(&path, "v3").unwrap();

        log.rollback().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "v1");
    }
}
