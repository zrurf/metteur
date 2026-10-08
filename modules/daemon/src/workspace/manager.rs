//! Workspace manager and active workspace registry.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use metteur_shared::config::Config;
use tokio::sync::RwLock;

use super::lock::SessionLock;
use crate::config;
use crate::error::{DaemonError, DaemonResult};
use crate::storage::persistence::Db;
use crate::storage::versioning::{VersionManager, WorkspaceWatcher};

/// The name of the workspace metadata directory.
pub const METADATA_DIR: &str = ".metteur";

/// A single open workspace.
pub struct Workspace {
    /// Serializes run admission and chat rewind for this workspace.
    pub activity_gate: tokio::sync::Mutex<()>,
    /// Absolute path to the workspace root.
    pub root: PathBuf,
    /// Path to the `/.metteur` metadata directory.
    #[allow(dead_code)]
    pub metadata_dir: PathBuf,
    /// The merged (global + workspace) configuration.
    pub config: Arc<RwLock<Config>>,
    pub(crate) applied_auto_snapshot: bool,
    /// The workspace-local database.
    pub db: Db,
    /// The workspace version manager (snapshots, file history).
    pub version_manager: Arc<VersionManager>,
    /// Language-server manager, rebuilt when the workspace LSP config changes.
    pub lsp_manager: crate::integration::lsp::SharedLsp,
    /// Background commands started by the agent in this workspace.
    pub jobs: Arc<crate::execution::JobManager>,
    /// The held session lock.
    _lock: SessionLock,
    /// The fs watcher feeding auto snapshots and live change events.
    watcher: Option<WorkspaceWatcher>,
}

impl Workspace {
    /// Admission holds activity_gate and verifies that no run is active first.
    pub fn reconcile_files(&self) -> DaemonResult<()> {
        crate::oversight::recovery::recover(&self.db)?;
        crate::execution::file_journal::FileJournal::new(
            self.root.clone(),
            Arc::new(crate::execution::file_journal::DbFileJournal(self.db.clone())),
        )
        .reconcile()?
        .ensure_safe()?;
        let conflicts = crate::replan::application::reconcile(&self.db, &self.version_manager)?;
        if !conflicts.is_empty() {
            return Err(DaemonError::Persistence(conflicts.join("; ")));
        }
        crate::replan::application::ensure_resolved(&self.db)?;
        Ok(())
    }
    /// Returns the workspace root path.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the live fs watcher, if one could be started.
    pub fn watcher(&self) -> Option<&WorkspaceWatcher> {
        self.watcher.as_ref()
    }

    /// Returns the language-server manager, if LSP is configured.
    pub fn lsp(&self) -> Option<Arc<crate::integration::lsp::LspManager>> {
        self.lsp_manager.read().clone()
    }

    /// Returns the workspace's background-command manager.
    pub fn jobs(&self) -> Arc<crate::execution::JobManager> {
        Arc::clone(&self.jobs)
    }

    /// Rebuilds the language-server manager from the current configuration.
    ///
    /// Returns whether a rebuild happened. The old manager is shut down
    /// asynchronously so its language-server processes do not outlive it;
    /// clients take a fresh snapshot through [`Self::lsp`] on each use.
    pub async fn reload_lsp(&self) -> bool {
        let lsp_config = self.config.read().await.lsp.clone();
        if !crate::integration::lsp::config_changed(&lsp_config, self.lsp_manager.read().as_deref())
        {
            return false;
        }
        let previous = {
            let mut slot = self.lsp_manager.write();
            let previous = slot.take();
            *slot = crate::integration::lsp::LspManager::new(&lsp_config, &self.root);
            previous
        };
        if let Some(previous) = previous {
            tokio::spawn(async move {
                previous.shutdown().await;
            });
        }
        true
    }
}

/// Manages the set of active workspaces.
#[derive(Default)]
pub struct WorkspaceManager {
    active: RwLock<HashMap<PathBuf, Arc<Workspace>>>,
    /// Global config file used when merging workspace configs (`--config`).
    global_config_path: Option<PathBuf>,
}

impl WorkspaceManager {
    /// Creates a new empty workspace manager.
    pub fn new() -> Self {
        Self::default()
    }

    /// Overrides the global config file used when merging workspace configs.
    pub fn with_global_config_path(mut self, path: PathBuf) -> Self {
        self.global_config_path = Some(path);
        self
    }

    /// Resolves the effective global config file path.
    pub fn global_config_path(&self) -> DaemonResult<PathBuf> {
        match &self.global_config_path {
            Some(path) => Ok(path.clone()),
            None => config::default_global_config_path(),
        }
    }

    /// Opens a workspace at `root`, creating its metadata directory if needed.
    ///
    /// If the directory is not already a workspace, the metadata directory is
    /// created (similar to `git init`). Returns an error if the workspace is
    /// already open or locked by another session.
    pub async fn open(&self, root: &Path) -> DaemonResult<Arc<Workspace>> {
        let root = normalize_path(&root.canonicalize().map_err(|e| {
            DaemonError::Io(std::io::Error::new(
                e.kind(),
                format!("cannot resolve workspace path {}: {e}", root.display()),
            ))
        })?);

        {
            let active = self.active.read().await;
            if let Some(ws) = active.get(&root) {
                return Ok(ws.clone());
            }
        }

        let metadata_dir = root.join(METADATA_DIR);
        let lock = SessionLock::acquire(&metadata_dir)?;
        let db = Db::open(&metadata_dir.join("db"))?;
        crate::oversight::recovery::recover(&db)?;
        let file_recovery = crate::execution::file_journal::FileJournal::new(
            root.clone(),
            Arc::new(crate::execution::file_journal::DbFileJournal(db.clone())),
        )
        .reconcile()?;
        if !file_recovery.conflicts.is_empty() {
            tracing::warn!(conflicts=?file_recovery.conflicts,"Workspace opened for inspection; file conflicts block execution admission");
        }
        let global_config_path = self.global_config_path()?;
        let config = config::load_merged_config(&global_config_path, &root)?;
        let version_manager = Arc::new(VersionManager::new(db.clone(), root.clone()));
        match crate::replan::application::reconcile(&db, &version_manager) {
            Ok(conflicts) => {
                for conflict in conflicts {
                    tracing::warn!(%conflict,"Blueprint application requires recovery before execution");
                }
            }
            Err(error) => {
                tracing::warn!(%error,"Blueprint recovery unavailable; execution admission will remain blocked")
            }
        }
        crate::oversight::conversation::recover(&db)?;
        let lsp_manager = crate::integration::lsp::LspManager::new(&config.lsp, &root);
        let watcher = match WorkspaceWatcher::start(
            root.clone(),
            version_manager.clone(),
            config.versioning.auto_snapshot,
        ) {
            Ok(watcher) => Some(watcher),
            Err(err) => {
                tracing::warn!("failed to start file watcher: {err}");
                None
            }
        };

        let workspace = Arc::new(Workspace {
            activity_gate: tokio::sync::Mutex::new(()),
            root: root.clone(),
            metadata_dir,
            applied_auto_snapshot: config.versioning.auto_snapshot,
            config: Arc::new(RwLock::new(config)),
            db,
            version_manager,
            jobs: Arc::new(crate::execution::JobManager::new(root.clone())),
            lsp_manager: Arc::new(parking_lot::RwLock::new(lsp_manager)),
            _lock: lock,
            watcher,
        });

        self.active.write().await.insert(root.clone(), workspace.clone());
        Ok(workspace)
    }

    /// Closes the workspace at `root`, releasing its lock.
    pub async fn close(&self, root: &Path) -> DaemonResult<()> {
        let root = normalize_path(&root.canonicalize().map_err(|e| {
            DaemonError::Io(std::io::Error::new(
                e.kind(),
                format!("cannot resolve workspace path {}: {e}", root.display()),
            ))
        })?);
        let removed = self.active.write().await.remove(&root);
        let Some(workspace) = removed else {
            return Err(DaemonError::NotFound(format!("workspace {} is not open", root.display())));
        };
        // Stop language servers before the session lock is released. The guard
        // is scoped so the lock is not held across the await.
        let lsp = workspace.lsp();
        if let Some(lsp) = lsp {
            lsp.shutdown().await;
        }
        // Background commands belong to the workspace: close it, close them.
        let killed = workspace.jobs().kill_all();
        if killed > 0 {
            tracing::info!("workspace {} closed; terminated {killed} job(s)", root.display());
        }
        Ok(())
    }

    /// Returns the open workspace at `root`, if any.
    pub async fn get(&self, root: &Path) -> Option<Arc<Workspace>> {
        let root = normalize_path(&root.canonicalize().ok()?);
        self.active.read().await.get(&root).cloned()
    }

    /// Returns all open workspaces.
    pub async fn list(&self) -> Vec<Arc<Workspace>> {
        self.active.read().await.values().cloned().collect()
    }
}

/// Converts a path to a normalized `PathBuf`.
///
/// On Windows, strips the `\\?\` extended-length prefix that `canonicalize`
/// may add, so that workspace paths compare and display consistently.
fn normalize_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let s = path.to_string_lossy();
        let stripped = s.strip_prefix(r"\\?\").unwrap_or(&s);
        PathBuf::from(stripped.to_string())
    }
    #[cfg(not(windows))]
    {
        path.to_path_buf()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn recovery_conflict_allows_inspection_but_blocks_execution() {
        use crate::execution::file_journal::{
            DbFileJournal, FileJournalStore, FileOperation, FileOrigin, FilePhase,
        };
        let dir =
            std::env::temp_dir().join(format!("metteur-ws-recovery-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join(METADATA_DIR)).unwrap();
        let path = dir.join("result.txt");
        std::fs::write(&path, "external edit").unwrap();
        {
            let store = DbFileJournal(Db::open(&dir.join(METADATA_DIR).join("db")).unwrap());
            store
                .save(&FileOperation {
                    operation_id: uuid::Uuid::new_v4(),
                    origin: FileOrigin {
                        run_id: uuid::Uuid::new_v4(),
                        node_id: uuid::Uuid::new_v4(),
                        attempt: 1,
                        wal_position: 0,
                    },
                    path: path.clone(),
                    before: Some(store.put_blob(b"before").unwrap()),
                    after: Some(store.put_blob(b"after").unwrap()),
                    phase: FilePhase::Prepared,
                })
                .unwrap();
        }
        let manager = WorkspaceManager::new();
        let ws = manager.open(&dir).await.unwrap();
        assert!(manager.get(&dir).await.is_some());
        assert!(ws.reconcile_files().is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "external edit");
        let operations = DbFileJournal(ws.db.clone()).operations().unwrap();
        assert_eq!(operations[0].phase, FilePhase::Conflict);
        manager.close(&dir).await.unwrap();
    }

    #[tokio::test]
    async fn opens_and_closes_workspace() {
        let dir = std::env::temp_dir().join(format!("metteur-ws-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let manager = WorkspaceManager::new();
        let _ws = manager.open(&dir).await.unwrap();
        assert!(dir.join(METADATA_DIR).exists());
        assert!(manager.get(&dir).await.is_some());
        manager.close(&dir).await.unwrap();
        assert!(manager.get(&dir).await.is_none());
    }

    #[tokio::test]
    async fn open_is_idempotent() {
        let dir = std::env::temp_dir().join(format!("metteur-ws-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let manager = WorkspaceManager::new();
        let a = manager.open(&dir).await.unwrap();
        let b = manager.open(&dir).await.unwrap();
        assert!(Arc::ptr_eq(&a, &b));
    }
}
