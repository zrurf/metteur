//! LSP client stack: JSON-RPC framing, per-language clients and the manager.

pub mod client;
pub mod debounce;
pub mod rpc;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use metteur_shared::config::LspConfig;
use tokio::sync::Mutex;

use crate::error::{DaemonError, DaemonResult};

use client::LspClient;

/// Live manager slot shared with running execution contexts.
pub type SharedLsp = Arc<parking_lot::RwLock<Option<Arc<LspManager>>>>;

/// Per-workspace manager of language server processes.
///
/// Clients are started lazily on the first request touching a file whose
/// extension maps to a configured language; all servers are shut down when
/// the workspace closes.
pub struct LspManager {
    workspace_root: PathBuf,
    languages: Vec<metteur_shared::config::LspLanguageConfig>,
    clients: Mutex<HashMap<String, ClientHandle>>,
    /// Snapshot of the config this manager was built from, used to decide
    /// whether a config change requires rebuilding it.
    config: LspConfig,
    /// Per-document sync cooldown merging rapid re-syncs.
    cooldown: tokio::sync::Mutex<crate::integration::lsp::debounce::SyncCooldown>,
    delegates: Vec<Arc<LspManager>>,
    owned: Option<OwnedOptions>,
    closed: std::sync::atomic::AtomicBool,
}

pub(crate) struct OwnedOptions {
    pub env: HashMap<String, HashMap<String, String>>,
    pub secrets: Vec<String>,
    pub timeout_ms: u64,
}

/// Returns whether a live manager no longer matches `config`.
///
/// Used to skip needless restarts: a config write that does not touch LSP
/// settings must not tear down running language servers.
pub fn config_changed(config: &LspConfig, current: Option<&LspManager>) -> bool {
    match current {
        Some(manager) => manager.config != *config,
        // No live manager: only a config that would produce one matters.
        None => config.enabled && !config.languages.is_empty(),
    }
}

struct ClientHandle {
    client: Arc<LspClient>,
    child: Arc<Mutex<Option<crate::integration::mcp::connection::OwnedChild>>>,
}

impl LspManager {
    /// Creates a manager when LSP is enabled and at least one language is
    /// configured; otherwise returns `None`.
    pub fn new(config: &LspConfig, workspace_root: &Path) -> Option<Arc<Self>> {
        if !config.enabled || config.languages.is_empty() {
            return None;
        }
        Some(Arc::new(Self {
            workspace_root: workspace_root.to_path_buf(),
            languages: config.languages.clone(),
            clients: Mutex::new(HashMap::new()),
            config: config.clone(),
            delegates: vec![],
            owned: None,
            closed: Default::default(),
            cooldown: tokio::sync::Mutex::new(
                crate::integration::lsp::debounce::SyncCooldown::new(
                    std::time::Duration::from_millis(config.debounce_ms),
                ),
            ),
        }))
    }

    pub(crate) fn owned(
        config: &LspConfig,
        root: &Path,
        options: OwnedOptions,
    ) -> Option<Arc<Self>> {
        let mut manager = Self::new(config, root)?;
        Arc::get_mut(&mut manager).expect("new manager").owned = Some(options);
        Some(manager)
    }

    /// User definitions are excluded from addon routes at admission. Keep those
    /// admitted routes pinned while other user languages retain live reloads.
    pub(crate) fn combine(user: Option<Arc<Self>>, addons: &[Arc<Self>]) -> Option<Arc<Self>> {
        if addons.is_empty() {
            return user;
        }
        let mut delegates = addons.to_vec();
        delegates.extend(user);
        let first = &delegates[0];
        let mut manager = Self::new(&first.config, &first.workspace_root)?;
        let inner = Arc::get_mut(&mut manager).expect("new router");
        inner.languages.clear();
        inner.delegates = delegates;
        Some(manager)
    }

    pub(crate) async fn healthy(&self) -> bool {
        !self.closed.load(std::sync::atomic::Ordering::SeqCst)
            && self.clients.lock().await.values().all(|h| !h.client.is_closed())
    }

    pub(crate) async fn initialize_all(&self) -> DaemonResult<()> {
        for language in &self.languages {
            if let Some(ext) = language.extensions.first() {
                self.client_for_extension(ext).await?;
            }
        }
        Ok(())
    }

    /// Returns the language id mapped to the given file extension.
    pub fn language_for_extension(&self, extension: &str) -> Option<String> {
        self.languages
            .iter()
            .find(|language| {
                language
                    .extensions
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(extension))
            })
            .map(|language| language.id.clone())
            .or_else(|| self.delegates.iter().find_map(|m| m.language_for_extension(extension)))
    }

    /// Returns the workspace root this manager is bound to.
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    /// Returns the effective document sync merge window.
    pub fn debounce(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.config.debounce_ms)
    }

    /// Returns whether a diagnostics pass should run when a node mutates files.
    pub fn check_on_node_end(&self) -> bool {
        self.config.check_on_node_end
    }

    /// Decides whether a document sync should run now.
    ///
    /// `explicit` callers (an LLM tool invocation or a validation node) always
    /// sync, because observing the current state is their entire purpose.
    /// Implicit callers within the merge window are told to reuse the most
    /// recent diagnostics instead of pushing an intermediate revision.
    pub async fn should_sync(
        &self,
        uri: &str,
        explicit: bool,
    ) -> crate::integration::lsp::debounce::SyncDecision {
        let now = std::time::Instant::now();
        let mut cooldown = self.document_manager(uri).cooldown.lock().await;
        cooldown.decide(uri, now, explicit)
    }

    /// Forgets the cooldown state of a document (its file disappeared).
    pub async fn forget_document(&self, uri: &str) {
        self.document_manager(uri).cooldown.lock().await.forget(uri);
    }

    fn document_manager(&self, uri: &str) -> &Self {
        let extension = metteur_shared::Uri::parse(uri)
            .and_then(|u| u.to_path())
            .ok()
            .and_then(|p| p.extension().map(|e| e.to_string_lossy().into_owned()))
            .unwrap_or_default();
        self.delegates
            .iter()
            .find(|m| m.language_for_extension(&extension).is_some())
            .map(Arc::as_ref)
            .unwrap_or(self)
    }

    /// Returns (starting it if needed) the client for a file extension.
    pub async fn client_for_extension(
        &self,
        extension: &str,
    ) -> DaemonResult<Option<Arc<LspClient>>> {
        let manager = self
            .delegates
            .iter()
            .find(|m| m.language_for_extension(extension).is_some())
            .map(Arc::as_ref)
            .unwrap_or(self);
        manager.local_client(extension).await
    }

    async fn local_client(&self, extension: &str) -> DaemonResult<Option<Arc<LspClient>>> {
        if self.closed.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(DaemonError::Lsp("language server has been retired".into()));
        }
        let Some(language_id) = self.language_for_extension(extension) else {
            return Ok(None);
        };
        let mut clients = self.clients.lock().await;
        if self.closed.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(DaemonError::Lsp("language server has been retired".into()));
        }
        if let Some(handle) = clients.get(&language_id) {
            if handle.client.is_closed() {
                return Err(DaemonError::Lsp("language server connection failed".into()));
            }
            return Ok(Some(handle.client.clone()));
        }
        let definition = self
            .languages
            .iter()
            .find(|language| language.id == language_id)
            .ok_or_else(|| DaemonError::Lsp(format!("language {language_id} vanished")))?;
        let (program, args) = definition.command.split_first().ok_or_else(|| {
            DaemonError::Lsp(format!("language {language_id} has an empty command"))
        })?;

        let mut command = tokio::process::Command::new(program);
        command
            .args(args)
            .current_dir(&self.workspace_root)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        if let Some(options) = &self.owned {
            command.env_clear();
            if let Some(env) = options.env.get(&language_id) {
                command.envs(env);
            }
        }
        #[cfg(unix)]
        command.process_group(0);
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let mut child = crate::integration::mcp::connection::OwnedChild(Some(
            command.spawn().map_err(|err| {
                DaemonError::Lsp(if self.owned.is_some() {
                    "failed to start addon language server".into()
                } else {
                    format!("failed to start {language_id} server: {err}")
                })
            })?,
        ));
        let process = child.0.as_mut().expect("owned child");
        let stdin = process
            .stdin
            .take()
            .ok_or_else(|| DaemonError::Lsp("server stdin unavailable".to_string()))?;
        let stdout = process
            .stdout
            .take()
            .ok_or_else(|| DaemonError::Lsp("server stdout unavailable".to_string()))?;

        let client = match &self.owned {
            Some(options) => LspClient::guarded(
                Box::new(stdout),
                Box::new(stdin),
                options.timeout_ms,
                options.secrets.clone(),
            ),
            None => LspClient::over_streams(Box::new(stdout), Box::new(stdin)),
        };
        if let Err(err) = client.initialize(&self.workspace_root).await {
            // Never leak the spawned server process on a failed handshake.
            client.shutdown().await;
            child.shutdown().await;
            return Err(err);
        }
        let child = Arc::new(Mutex::new(Some(child)));
        let watched_child = child.clone();
        let watched_client = Arc::downgrade(&client);
        tokio::spawn(async move {
            loop {
                if watched_client.upgrade().is_none_or(|client| client.is_closed()) {
                    if let Some(child) = watched_child.lock().await.take() {
                        child.shutdown().await;
                    }
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        });
        clients.insert(
            language_id,
            ClientHandle {
                client: client.clone(),
                child,
            },
        );
        Ok(Some(client))
    }

    /// Shuts down every running language server.
    pub async fn shutdown(&self) {
        self.closed.store(true, std::sync::atomic::Ordering::SeqCst);
        let mut clients = self.clients.lock().await;
        for (_, handle) in clients.drain() {
            handle.client.shutdown().await;
            if let Some(child) = handle.child.lock().await.take() {
                child.shutdown().await;
            }
        }
    }

    /// Converts a workspace-relative path into a `file://` URI.
    pub fn to_file_uri(&self, absolute: &Path) -> String {
        metteur_shared::Uri::from_path(absolute).to_string()
    }
}
