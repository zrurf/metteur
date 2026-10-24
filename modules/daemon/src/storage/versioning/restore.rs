//! Preflight and compensating restore. Never rewrite unchanged files: on Windows
//! a readable executable/Git object can still be unwritable while in use.
use std::collections::BTreeSet;
use std::fs::{File, OpenOptions, Permissions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

use uuid::Uuid;

use super::{Snapshot, VersionManager};
use crate::error::{DaemonError, DaemonResult};
use crate::storage::persistence::cf;

/// Apply the same exclusions to new scans AND old manifests. Otherwise an old
/// snapshot would still overwrite running binaries or delete Git metadata.
pub(super) fn excluded(path: &Path) -> bool {
    path.components().any(|part| {
        matches!(
            part.as_os_str().to_string_lossy().to_ascii_lowercase().as_str(),
            ".metteur"
                | ".git"
                | ".hg"
                | ".svn"
                | "node_modules"
                | "target"
                | "dist"
                | "build"
                | ".next"
                | ".nuxt"
                | ".cache"
                | "__pycache__"
                | ".venv"
        )
    })
}

pub(super) use crate::workspace::fs::is_link;

pub(super) fn io_at(operation: &str, path: &Path, error: std::io::Error) -> DaemonError {
    DaemonError::Io(std::io::Error::new(
        error.kind(),
        format!("{operation} '{}': {error}", path.display()),
    ))
}

fn relative(raw: &str) -> DaemonResult<PathBuf> {
    let normalized = raw.replace('\\', "/");
    let path = PathBuf::from(&normalized);
    if normalized.is_empty()
        || normalized.contains(':')
        || normalized.starts_with('/')
        || normalized.split('/').any(|part| part == ".." || part == "." || part.is_empty())
        || path.components().any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(DaemonError::PermissionDenied(format!("unsafe snapshot path: {raw}")));
    }
    Ok(path)
}

/// Reject link/junction traversal, including a symlink introduced AFTER capture.
fn safe_target(root: &Path, relative: &Path) -> DaemonResult<PathBuf> {
    let mut target = root.to_path_buf();
    for component in relative.components() {
        target.push(component.as_os_str());
        match std::fs::symlink_metadata(&target) {
            Ok(metadata) if is_link(&metadata) => {
                return Err(DaemonError::PermissionDenied(format!(
                    "restore refuses symbolic link or junction '{}'",
                    target.display()
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_at("inspect restore path", &target, error)),
        }
    }
    Ok(target)
}

struct Change {
    relative: PathBuf,
    path: PathBuf,
    before: Option<Vec<u8>>,
    after: Option<Vec<u8>>,
    permissions: Option<Permissions>,
    /// Opened without truncation during preflight; verifies write access before
    /// any other file is changed and reserves it against other Windows writers.
    writer: Option<File>,
    touched: bool,
}

impl VersionManager {
    /// Restores file differences and commits associated model state. Preflight
    /// errors change nothing; later failures undo only files actually touched.
    /// This is not a cross-filesystem/database crash-atomic transaction.
    pub fn rollback_with_commit<T>(
        &self,
        snapshot_id: Uuid,
        commit: impl FnOnce() -> DaemonResult<T>,
    ) -> DaemonResult<T> {
        crate::replan::application::ensure_resolved(&self.db)?;
        let _guard = self.operation_gate.lock();
        let encoded = self
            .db
            .get(cf::SNAPSHOTS, snapshot_id.as_bytes())?
            .ok_or_else(|| DaemonError::NotFound(format!("snapshot {snapshot_id}")))?;
        let snapshot: Snapshot = serde_json::from_slice(&encoded)
            .map_err(|error| DaemonError::Serialization(error.to_string()))?;
        let mut changes = self.prepare_restore(&snapshot).map_err(|error| {
            DaemonError::Execution(format!("restore preflight failed; no files changed: {error}"))
        })?;
        // Build under the same gate; calling create_snapshot would re-lock it.
        let (recovery, _) = self.build_snapshot("Before restore (recovery)", None)?;
        self.store_snapshot(&recovery)?;
        let mut directories = Vec::new();
        let outcome = self.apply_restore(&mut changes, &mut directories).and_then(|_| commit());
        match outcome {
            Ok(value) => Ok(value),
            Err(error) => {
                let mut failures = Vec::new();
                for change in changes.iter_mut().rev().filter(|change| change.touched) {
                    if let Err(error) = undo(change) {
                        failures.push(error.to_string());
                    }
                }
                for directory in directories.iter().rev() {
                    // Remove only empty directories created by this operation.
                    let _ = std::fs::remove_dir(directory);
                }
                let status = if failures.is_empty() {
                    "previous files restored".to_string()
                } else {
                    format!("recovery failed for changed files: {}", failures.join("; "))
                };
                Err(DaemonError::Execution(format!(
                    "{error}; {status}; recovery snapshot: {}",
                    recovery.id
                )))
            }
        }
    }

    fn prepare_restore(&self, snapshot: &Snapshot) -> DaemonResult<Vec<Change>> {
        let mut paths: BTreeSet<String> = self.all_tracked_files()?.into_iter().collect();
        paths.extend(snapshot.files.keys().cloned());
        // New files not yet seen by the watcher must also be removed on rewind.
        for path in self.collect_files(&self.root)? {
            paths.insert(
                path.strip_prefix(&self.root).unwrap().to_string_lossy().replace('\\', "/"),
            );
        }
        let mut changes = Vec::new();
        for raw in paths {
            let relative = relative(&raw)?;
            if excluded(&relative) {
                continue;
            }
            let path = safe_target(&self.root, &relative)?;
            let metadata = match std::fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.is_file() => Some(metadata),
                Ok(_) => {
                    return Err(DaemonError::Execution(format!(
                        "restore expected a regular file at '{}'",
                        path.display()
                    )));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(io_at("inspect restore file", &path, error)),
            };
            let before = metadata
                .as_ref()
                .map(|_| {
                    std::fs::read(&path).map_err(|error| io_at("read current file", &path, error))
                })
                .transpose()?;
            let after = snapshot
                .files
                .get(&raw)
                .map(|hash| {
                    self.db.get(cf::FILE_BLOBS, hash.as_bytes())?.ok_or_else(|| {
                        DaemonError::NotFound(format!(
                            "snapshot blob {hash} for '{}'",
                            path.display()
                        ))
                    })
                })
                .transpose()?;
            if before == after {
                continue;
            }
            let permissions = metadata.as_ref().map(|metadata| metadata.permissions());
            if permissions.as_ref().is_some_and(Permissions::readonly) {
                return Err(DaemonError::PermissionDenied(format!(
                    "restore needs to change read-only file '{}'; make this file writable before retrying",
                    path.display()
                )));
            }
            let writer = if before.is_some() {
                let mut options = OpenOptions::new();
                options.read(true).write(true); // deliberately NOT truncate(true)
                #[cfg(windows)]
                {
                    use std::os::windows::fs::OpenOptionsExt;
                    options.share_mode(0x1 | 0x4); // FILE_SHARE_READ | FILE_SHARE_DELETE
                }
                Some(options.open(&path).map_err(|error| {
                    io_at("open restore file for writing (close programs using it)", &path, error)
                })?)
            } else {
                None
            };
            changes.push(Change {
                relative,
                path,
                before,
                after,
                permissions,
                writer,
                touched: false,
            });
        }
        Ok(changes)
    }

    fn apply_restore(
        &self,
        changes: &mut [Change],
        directories: &mut Vec<PathBuf>,
    ) -> DaemonResult<()> {
        for change in changes {
            safe_target(&self.root, &change.relative)?;
            if let Some(after) = &change.after {
                if change.writer.is_none() {
                    create_parents(&change.path, directories)?;
                    let file =
                        OpenOptions::new()
                            .write(true)
                            .create_new(true)
                            .open(&change.path)
                            .map_err(|error| io_at("create restored file", &change.path, error))?;
                    change.writer = Some(file);
                }
                change.touched = true; // even a partial write must be undone
                write_contents(change.writer.as_mut().unwrap(), after)
                    .map_err(|error| io_at("write restored file", &change.path, error))?;
            } else {
                std::fs::remove_file(&change.path).map_err(|error| {
                    io_at("delete file absent from snapshot", &change.path, error)
                })?;
                change.touched = true;
                // Windows cannot recreate a delete-pending file until closed.
                change.writer.take();
            }
        }
        Ok(())
    }
}

fn write_contents(file: &mut File, content: &[u8]) -> std::io::Result<()> {
    file.seek(SeekFrom::Start(0))?;
    file.write_all(content)?;
    file.set_len(content.len() as u64)?;
    file.sync_all()
}

fn create_parents(path: &Path, created: &mut Vec<PathBuf>) -> DaemonResult<()> {
    let mut missing = Vec::new();
    let mut parent = path.parent();
    while let Some(directory) = parent {
        if directory.exists() {
            break;
        }
        missing.push(directory.to_path_buf());
        parent = directory.parent();
    }
    for directory in missing.into_iter().rev() {
        std::fs::create_dir(&directory)
            .map_err(|error| io_at("create restore directory", &directory, error))?;
        created.push(directory);
    }
    Ok(())
}

fn undo(change: &mut Change) -> DaemonResult<()> {
    if let Some(before) = &change.before {
        let recreated = change.writer.is_none();
        if change.writer.is_none() {
            change.writer =
                Some(OpenOptions::new().write(true).create_new(true).open(&change.path).map_err(
                    |error| io_at("recreate file during recovery", &change.path, error),
                )?);
        }
        write_contents(change.writer.as_mut().unwrap(), before)
            .map_err(|error| io_at("recover changed file", &change.path, error))?;
        if let Some(permissions) = &change.permissions
            && recreated
        {
            std::fs::set_permissions(&change.path, permissions.clone())
                .map_err(|error| io_at("recover file permissions", &change.path, error))?;
        }
    } else {
        change.writer.take();
        std::fs::remove_file(&change.path).map_err(|error| {
            io_at("remove newly created file during recovery", &change.path, error)
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::persistence::Db;

    fn workspace() -> (PathBuf, VersionManager) {
        let root = std::env::temp_dir().join(format!("metteur-safe-restore-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let db = Db::open(&root.join(".metteur/db")).unwrap();
        let manager = VersionManager::new(db, root.clone());
        (root, manager)
    }

    #[test]
    fn exclusions_apply_to_capture_legacy_restore_and_legacy_deletion() {
        let (root, manager) = workspace();
        std::fs::write(root.join("code.txt"), "before").unwrap();
        for path in [
            ".git/objects/pack.obj",
            "node_modules/pkg/index.js",
            "target/debug/app.exe",
            "dist/app.js",
        ] {
            std::fs::create_dir_all(root.join(path).parent().unwrap()).unwrap();
            std::fs::write(root.join(path), "must not touch").unwrap();
        }
        let clean = manager.create_snapshot("source only").unwrap();
        assert_eq!(clean.files.len(), 1);
        let mut legacy = clean.clone();
        legacy.id = Uuid::new_v4();
        // Old artifact blobs may even be missing; they are never read/restored.
        legacy.files.insert(".git/objects/pack.obj".into(), "missing".into());
        legacy.files.insert("target/debug/app.exe".into(), "missing".into());
        manager.store_snapshot(&legacy).unwrap();
        std::fs::write(root.join("code.txt"), "after").unwrap();
        manager.rollback(legacy.id).unwrap();
        assert_eq!(std::fs::read_to_string(root.join("code.txt")).unwrap(), "before");
        manager.rollback(clean.id).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join(".git/objects/pack.obj")).unwrap(),
            "must not touch"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("target/debug/app.exe")).unwrap(),
            "must not touch"
        );
    }

    #[test]
    fn unchanged_read_only_file_is_not_rewritten() {
        let (root, manager) = workspace();
        std::fs::write(root.join("a.txt"), "before").unwrap();
        std::fs::write(root.join("locked.txt"), "unchanged").unwrap();
        let snapshot = manager.create_snapshot("initial").unwrap();
        std::fs::write(root.join("a.txt"), "after").unwrap();
        let mut permissions = std::fs::metadata(root.join("locked.txt")).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(root.join("locked.txt"), permissions).unwrap();
        manager.rollback(snapshot.id).unwrap();
        assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "before");
        assert!(std::fs::metadata(root.join("locked.txt")).unwrap().permissions().readonly());
    }

    #[test]
    fn changed_read_only_file_fails_preflight_before_other_files_change() {
        let (root, manager) = workspace();
        for name in ["a.txt", "z-locked.txt"] {
            std::fs::write(root.join(name), "before").unwrap();
        }
        let snapshot = manager.create_snapshot("initial").unwrap();
        for name in ["a.txt", "z-locked.txt"] {
            std::fs::write(root.join(name), "after").unwrap();
        }
        let mut permissions = std::fs::metadata(root.join("z-locked.txt")).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(root.join("z-locked.txt"), permissions).unwrap();
        let error = manager.rollback(snapshot.id).unwrap_err().to_string();
        assert!(error.contains("no files changed") && error.contains("z-locked.txt"), "{error}");
        assert!(!error.contains("recovery failed"));
        assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "after");
    }

    #[test]
    fn commit_failure_compensates_changes_without_touching_unchanged_read_only_files() {
        let (root, manager) = workspace();
        std::fs::write(root.join("a.txt"), "before").unwrap();
        std::fs::write(root.join("locked.txt"), "same").unwrap();
        let snapshot = manager.create_snapshot("initial").unwrap();
        std::fs::write(root.join("a.txt"), "after").unwrap();
        std::fs::write(root.join("new.txt"), "new file must survive failed commit").unwrap();
        let mut permissions = std::fs::metadata(root.join("locked.txt")).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(root.join("locked.txt"), permissions).unwrap();
        let result: DaemonResult<()> = manager.rollback_with_commit(snapshot.id, || {
            Err(DaemonError::Internal("commit failed".into()))
        });
        let error = result.unwrap_err().to_string();
        assert!(error.contains("previous files restored"), "{error}");
        assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "after");
        assert_eq!(
            std::fs::read_to_string(root.join("new.txt")).unwrap(),
            "new file must survive failed commit"
        );
        assert!(std::fs::metadata(root.join("locked.txt")).unwrap().permissions().readonly());
    }

    #[test]
    fn rejects_unsafe_legacy_paths_before_mutation() {
        let (root, manager) = workspace();
        std::fs::write(root.join("a.txt"), "before").unwrap();
        let snapshot = manager.create_snapshot("initial").unwrap();
        std::fs::write(root.join("a.txt"), "after").unwrap();
        for bad in ["../escape.txt", "C:/outside.txt", "a.txt:stream"] {
            let mut legacy = snapshot.clone();
            legacy.id = Uuid::new_v4();
            legacy.files.insert(bad.into(), "missing".into());
            manager.store_snapshot(&legacy).unwrap();
            assert!(manager.rollback(legacy.id).is_err());
            assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "after");
            manager.db.delete(cf::SNAPSHOTS, legacy.id.as_bytes()).unwrap();
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_sharing_violation_is_preflighted_and_unchanged_open_files_are_skipped() {
        use std::os::windows::fs::OpenOptionsExt;
        let (root, manager) = workspace();
        std::fs::write(root.join("a.txt"), "before").unwrap();
        std::fs::write(root.join("z-running.exe"), "original executable").unwrap();
        let initial = manager.create_snapshot("original").unwrap();
        std::fs::write(root.join("a.txt"), "after").unwrap();
        let handle =
            OpenOptions::new().read(true).share_mode(1).open(root.join("z-running.exe")).unwrap();
        manager.rollback(initial.id).unwrap();
        assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "before");
        drop(handle);
        std::fs::write(root.join("a.txt"), "after again").unwrap();
        std::fs::write(root.join("z-running.exe"), "changed executable").unwrap();
        let _handle =
            OpenOptions::new().read(true).share_mode(1).open(root.join("z-running.exe")).unwrap();
        let error = manager.rollback(initial.id).unwrap_err().to_string();
        assert!(error.contains("no files changed") && error.contains("z-running.exe"), "{error}");
        assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "after again");
    }

    #[cfg(unix)]
    #[test]
    fn skips_symlinks_at_capture_and_rejects_links_introduced_after_capture() {
        use std::os::unix::fs::symlink;
        let (root, manager) = workspace();
        let outside = std::env::temp_dir().join(format!("metteur-outside-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret"), "outside").unwrap();
        symlink(&outside, root.join("linked")).unwrap();
        std::fs::write(root.join("source.txt"), "before").unwrap();
        let snapshot = manager.create_snapshot("no links").unwrap();
        assert_eq!(snapshot.files.len(), 1);
        std::fs::remove_file(root.join("source.txt")).unwrap();
        symlink(outside.join("secret"), root.join("source.txt")).unwrap();
        assert!(manager.rollback(snapshot.id).is_err());
        assert_eq!(std::fs::read_to_string(outside.join("secret")).unwrap(), "outside");
    }

    #[cfg(windows)]
    #[test]
    fn windows_junctions_are_not_captured_or_followed_during_restore() {
        use std::os::windows::process::CommandExt;
        let (root, manager) = workspace();
        let outside =
            std::env::temp_dir().join(format!("metteur-junction-target-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("source.txt"), "outside must not change").unwrap();
        let junction = |link: &Path| {
            let output = std::process::Command::new("cmd.exe")
                .args(["/C", "mklink", "/J"])
                .arg(link)
                .arg(&outside)
                .creation_flags(0x08000000)
                .output()
                .unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        };
        junction(&root.join("linked"));
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("src/source.txt"), "before").unwrap();
        let snapshot = manager.create_snapshot("ignore junctions").unwrap();
        assert_eq!(snapshot.files.len(), 1);
        std::fs::remove_file(root.join("src/source.txt")).unwrap();
        std::fs::remove_dir(root.join("src")).unwrap();
        junction(&root.join("src"));
        let error = manager.rollback(snapshot.id).unwrap_err().to_string();
        assert!(error.contains("junction") && error.contains("no files changed"), "{error}");
        assert_eq!(
            std::fs::read_to_string(outside.join("source.txt")).unwrap(),
            "outside must not change"
        );
    }
}
