//! Automatic version snapshots driven by file system events.
//!
//! A [`WorkspaceWatcher`] watches a workspace root with `notify`, debounces
//! file changes and — when auto-snapshots are enabled — creates a snapshot
//! once the tree goes quiet. Every relevant change is also broadcast so
//! clients can react live (e.g. refresh the explorer or warn about a file
//! edited on disk).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::{broadcast, oneshot};
use tokio::task::JoinHandle;

use crate::error::{DaemonError, DaemonResult};
use crate::storage::versioning::VersionManager;

/// How long after the last file event a snapshot is taken.
const DEBOUNCE: Duration = Duration::from_secs(2);

/// The kind of a file-system change surfaced to subscribers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchKind {
    Created,
    Modified,
    Removed,
}

/// A change event for a file inside the watched workspace.
#[derive(Debug, Clone)]
pub struct FileEvent {
    pub path: PathBuf,
    pub kind: WatchKind,
}

/// A per-workspace watcher that snapshots changed files after a quiet period
/// and broadcasts every relevant change to subscribers.
pub struct WorkspaceWatcher {
    stop: Option<oneshot::Sender<()>>,
    handle: JoinHandle<()>,
    events: broadcast::Sender<FileEvent>,
}

impl WorkspaceWatcher {
    /// Starts watching `root`; snapshots only happen when `auto_snapshot`.
    pub fn start(
        root: PathBuf,
        manager: Arc<VersionManager>,
        auto_snapshot: bool,
    ) -> DaemonResult<Self> {
        let (stop_tx, stop_rx) = oneshot::channel();
        let (event_tx, event_rx) = std::sync::mpsc::channel::<notify::Result<Event>>();
        let (events_tx, _) = broadcast::channel(256);

        let mut watcher = RecommendedWatcher::new(
            move |res| {
                let _ = event_tx.send(res);
            },
            notify::Config::default(),
        )
        .map_err(|e| DaemonError::Io(std::io::Error::other(e.to_string())))?;
        watch_recursively(&mut watcher, &root).map_err(DaemonError::Io)?;

        let handle = tokio::spawn(run_loop(
            watcher,
            event_rx,
            stop_rx,
            root,
            manager,
            auto_snapshot,
            events_tx.clone(),
        ));
        Ok(Self {
            stop: Some(stop_tx),
            handle,
            events: events_tx,
        })
    }

    /// Receives every workspace change broadcast from the watcher loop.
    pub fn subscribe(&self) -> broadcast::Receiver<FileEvent> {
        self.events.subscribe()
    }
    pub async fn shutdown(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        self.handle.abort();
        let _ = (&mut self.handle).await;
    }
}

impl Drop for WorkspaceWatcher {
    fn drop(&mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
        self.handle.abort();
    }
}

/// Maps a notify event to a broadcast kind and relative path, if relevant.
fn to_file_event(root: &Path, event: &Event) -> Option<FileEvent> {
    let kind = match event.kind {
        EventKind::Create(_) => WatchKind::Created,
        EventKind::Remove(_) => WatchKind::Removed,
        EventKind::Modify(_) => WatchKind::Modified,
        _ => return None,
    };
    // Prefer the first path inside the workspace.
    let path = event.paths.iter().find(|p| {
        p.strip_prefix(root).map(|rel| !super::restore::excluded(rel)).unwrap_or(false)
    })?;
    Some(FileEvent {
        path: path.clone(),
        kind,
    })
}

/// The event-processing loop: broadcasts changes and debounces snapshots.
#[allow(clippy::too_many_arguments)]
async fn run_loop(
    mut watcher: RecommendedWatcher,
    event_rx: std::sync::mpsc::Receiver<notify::Result<Event>>,
    mut stop_rx: oneshot::Receiver<()>,
    root: PathBuf,
    manager: Arc<VersionManager>,
    auto_snapshot: bool,
    events: broadcast::Sender<FileEvent>,
) {
    let mut pending = false;
    let mut last_event = Instant::now();
    let tick = tokio::time::sleep(Duration::from_millis(100));
    tokio::pin!(tick);

    loop {
        tokio::select! {
            _ = &mut stop_rx => break,
            _ = &mut tick => {
                tick.as_mut().reset(tokio::time::Instant::now() + Duration::from_millis(100));
            }
        }

        // Drain queued events and extend watches for created directories.
        while let Ok(res) = event_rx.try_recv() {
            if let Ok(event) = res {
                if auto_snapshot && is_relevant(&root, &event) {
                    pending = true;
                    last_event = Instant::now();
                }
                if matches!(event.kind, EventKind::Create(_)) {
                    for path in &event.paths {
                        if path.is_dir()
                            && path
                                .strip_prefix(&root)
                                .is_ok_and(|rel| !super::restore::excluded(rel))
                        {
                            let _ = watch_recursively(&mut watcher, path);
                        }
                    }
                }
                if let Some(ev) = to_file_event(&root, &event) {
                    let _ = events.send(ev);
                }
            }
        }

        if pending && last_event.elapsed() >= DEBOUNCE {
            pending = false;
            match manager.create_snapshot_if_changed("auto") {
                Ok(Some(snapshot)) => {
                    tracing::info!("auto snapshot {} created", snapshot.id);
                }
                Ok(None) => {}
                Err(err) => tracing::warn!("auto snapshot failed: {err}"),
            }
        }
    }
}

/// Returns whether an event concerns a path inside `root` outside `.metteur`.
fn is_relevant(root: &Path, event: &Event) -> bool {
    if matches!(event.kind, EventKind::Access(_)) {
        return false;
    }
    event
        .paths
        .iter()
        .any(|p| p.strip_prefix(root).map(|rel| !super::restore::excluded(rel)).unwrap_or(false))
}

/// Returns whether a directory belongs to metadata or generated artifacts.
fn is_metadata_dir(path: &Path) -> bool {
    path.file_name().map(|name| super::restore::excluded(Path::new(name))).unwrap_or(false)
}

/// Recursively adds non-recursive watches for `dir` and its subdirectories.
fn watch_recursively(watcher: &mut RecommendedWatcher, dir: &Path) -> std::io::Result<()> {
    if super::restore::is_link(&std::fs::symlink_metadata(dir)?) {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !is_metadata_dir(&path)
            && entry.file_type()?.is_dir()
            && !super::restore::is_link(&std::fs::symlink_metadata(&path)?)
        {
            watch_recursively(watcher, &path)?;
        }
    }
    watcher
        .watch(dir, RecursiveMode::NonRecursive)
        .map_err(|e| std::io::Error::other(e.to_string()))
}
