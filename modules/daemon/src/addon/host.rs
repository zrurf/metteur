//! Scoped package admission and reconciliation. Installed bytes do not grant authority.
use super::{
    AddonTool,
    manifest::Manifest,
    package::{Identity, Package},
    runtime::Permission,
    signature::{Policy, Snapshot},
};
use crate::{
    DaemonError, DaemonResult,
    registry::{Registry, Tool},
};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::{Mutex, RwLock as AsyncRwLock};

#[derive(Clone)]
struct Loaded {
    package: Arc<Package>,
    permissions: HashSet<Permission>,
}
#[derive(Serialize, Deserialize)]
struct Receipt {
    identity: Identity,
    granted: Vec<String>,
    enabled: bool,
}
#[derive(Debug, Clone)]
pub struct AddonInfoData {
    pub id: String,
    pub version: String,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub scope: String,
    pub scope_root: String,
    pub fingerprint: String,
    pub status: String,
    pub error: String,
    pub required_permissions: Vec<String>,
    pub granted_permissions: Vec<String>,
    pub tool_count: u32,
    pub fragment_count: u32,
    pub hooks: Vec<super::hooks::Report>,
}
pub struct AddonHost {
    global_dir: PathBuf,
    state_dir: PathBuf,
    registry: Arc<Registry>,
    fallback_timeout_ms: u64,
    signature: Policy,
    loaded: RwLock<BTreeMap<String, Loaded>>,
    errors: RwLock<BTreeMap<String, String>>,
    roots: RwLock<BTreeSet<PathBuf>>,
    closed_roots: RwLock<BTreeSet<PathBuf>>,
    maintenance: Mutex<()>,
    lifecycle: Arc<AsyncRwLock<()>>,
    services: Arc<Mutex<super::services::Services>>,
    hooks: Arc<super::hooks::Bus>,
}
fn failure(message: impl Into<String>) -> DaemonError {
    DaemonError::Addon(message.into())
}
fn scope(root: Option<&Path>) -> String {
    root.map(|r| r.to_string_lossy().into_owned()).unwrap_or_else(|| "global".into())
}
fn owner(root: Option<&Path>, id: &str) -> String {
    format!("{}::{id}", scope(root))
}
fn workspace_root(root: &Path) -> DaemonResult<PathBuf> {
    Ok(crate::workspace::fs::WorkspaceFs::new(root.canonicalize()?).root().to_path_buf())
}
fn ordered(entries: Vec<Loaded>) -> Vec<Loaded> {
    let manifests: Vec<_> = entries.iter().map(|e| &e.package.manifest).collect();
    super::functions::order(&manifests).into_iter().map(|i| entries[i].clone()).collect()
}

impl AddonHost {
    pub fn new(
        data_dir: &Path,
        registry: Arc<Registry>,
        timeout: u64,
        config: &metteur_shared::config::AddonConfig,
    ) -> Arc<Self> {
        Arc::new(Self {
            global_dir: data_dir.join("addons"),
            state_dir: data_dir.join("addon-state"),
            registry,
            fallback_timeout_ms: timeout,
            signature: Policy::from_config(config),
            loaded: Default::default(),
            errors: Default::default(),
            roots: Default::default(),
            closed_roots: Default::default(),
            maintenance: Mutex::new(()),
            lifecycle: Arc::new(AsyncRwLock::new(())),
            services: Default::default(),
            hooks: Default::default(),
        })
    }
    fn directory(&self, root: Option<&Path>) -> PathBuf {
        root.map(|r| r.join(".metteur/addons")).unwrap_or_else(|| self.global_dir.clone())
    }
    fn receipt_path(&self, owner: &str) -> PathBuf {
        use sha2::{Digest, Sha256};
        let hash: String =
            Sha256::digest(owner.as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect();
        self.state_dir.join(format!("{hash}.toml"))
    }
    fn receipt(&self, package: &Package) -> DaemonResult<Receipt> {
        let path = self.receipt_path(&package.identity.owner());
        if !path.exists() {
            if !package.manifest.permissions.required.is_empty() {
                return Err(failure(
                    "Explicit grants required; legacy package grant files are not authorization receipts",
                ));
            }
            return Ok(Receipt {
                identity: package.identity.clone(),
                granted: vec![],
                enabled: true,
            });
        }
        let receipt: Receipt = toml::from_str(&std::fs::read_to_string(path)?)
            .map_err(|_| failure("Invalid addon authorization receipt"))?;
        if receipt.identity != package.identity {
            return Err(failure(
                "Addon content or signature changed; reinstall with explicit grants",
            ));
        }
        Ok(receipt)
    }
    fn tools(&self, loaded: &Loaded) -> Vec<Arc<dyn Tool>> {
        loaded
            .package
            .manifest
            .tools
            .iter()
            .map(|entry| {
                Arc::new(AddonTool::new(
                    loaded.package.clone(),
                    entry,
                    loaded.permissions.clone(),
                    self.fallback_timeout_ms,
                )) as Arc<dyn Tool>
            })
            .collect()
    }
    fn install_into(&self, registry: &mut Registry, loaded: &Loaded) -> DaemonResult<()> {
        let mut candidate = registry.snapshot();
        self.install_contributions(&mut candidate, loaded)?;
        *registry = candidate;
        Ok(())
    }
    fn install_contributions(&self, registry: &mut Registry, loaded: &Loaded) -> DaemonResult<()> {
        let identity = &loaded.package.identity;
        registry.remove_owned_functions(&identity.owner());
        for language in &loaded.package.manifest.lsp {
            for extension in &language.language.extensions {
                let key = extension.to_ascii_lowercase();
                if registry
                    .addon_lsp_claims
                    .get(&key)
                    .is_some_and(|owner| owner != &identity.owner())
                {
                    return Err(failure("Addon language extension conflicts with another package"));
                }
            }
        }
        if registry
            .addon_packages
            .values()
            .any(|p| p.id == identity.id && p.owner() != identity.owner())
        {
            return Err(failure("Workspace addon cannot shadow a global addon identity"));
        }
        registry.replace_owned_tools(&identity.owner(), self.tools(loaded))?;
        registry.replace_owned_nodes(
            &identity.owner(),
            loaded
                .package
                .manifest
                .nodes
                .iter()
                .map(|entry| {
                    Arc::new(super::nodes::AddonNode::new(
                        loaded.package.clone(),
                        entry,
                        loaded.permissions.clone(),
                        self.fallback_timeout_ms,
                    )) as Arc<dyn crate::registry::NodeExecutor>
                })
                .collect(),
            serde_json::to_value(identity).map_err(|_| failure("Invalid addon node identity"))?,
        )?;
        for language in &loaded.package.manifest.lsp {
            for extension in &language.language.extensions {
                registry.addon_lsp_claims.insert(extension.to_ascii_lowercase(), identity.owner());
            }
        }
        super::functions::install(registry, &loaded.package)?;
        registry.addon_packages.insert(identity.owner(), identity.clone());
        // Keep global fragments before workspace fragments at equal priority,
        // matching the existing prompt assembly order.
        let fragment_owner = format!(
            "{}:{}",
            if identity.scope == "global" {
                0
            } else {
                1
            },
            identity.owner()
        );
        registry.addon_fragments.insert(fragment_owner, loaded.package.fragments.clone());
        Ok(())
    }
    fn snapshot_locked(&self, root: Option<&Path>) -> DaemonResult<Registry> {
        let mut registry = self.registry.addon_base_snapshot();
        let loaded = self.loaded.read();
        for entry in ordered(
            loaded
                .values()
                .filter(|entry| entry.package.identity.scope == "global")
                .cloned()
                .collect(),
        ) {
            self.install_into(&mut registry, &entry)?;
        }
        if let Some(root) = root {
            for entry in ordered(
                loaded
                    .values()
                    .filter(|entry| entry.package.identity.scope == scope(Some(root)))
                    .cloned()
                    .collect(),
            ) {
                self.install_into(&mut registry, &entry)?;
            }
        }
        Ok(registry)
    }
    fn refresh_locked(&self) {
        let previous = std::mem::take(&mut *self.loaded.write());
        for entry in previous.values() {
            if entry.package.identity.scope == "global" {
                self.registry.remove_owned_tools(&entry.package.identity.owner());
            }
        }
        let roots: Vec<_> =
            std::iter::once(None).chain(self.roots.read().iter().cloned().map(Some)).collect();
        let mut loaded = BTreeMap::<String, Loaded>::new();
        let mut errors = BTreeMap::new();
        for root in roots {
            let dirs = super::functions::order_dirs(package_dirs(&self.directory(root.as_deref())));
            let mut ids = BTreeMap::<String, usize>::new();
            for dir in &dirs {
                if let Ok((manifest, _)) = Manifest::load(dir) {
                    *ids.entry(manifest.id).or_default() += 1;
                }
            }
            let mut scoped = self.registry.addon_base_snapshot();
            if root.is_some() {
                for entry in ordered(loaded.values().cloned().collect()) {
                    if entry.package.identity.scope == "global" {
                        let _ = self.install_into(&mut scoped, &entry);
                    }
                }
            }
            for dir in dirs {
                let fallback = dir.file_name().unwrap_or_default().to_string_lossy().into_owned();
                let id = Manifest::load(&dir).map(|(m, _)| m.id).unwrap_or(fallback);
                let key = owner(root.as_deref(), &id);
                let result = (|| -> DaemonResult<Option<Loaded>> {
                    if ids.get(&id).copied().unwrap_or(0) > 1 {
                        return Err(failure("Duplicate addon identity in this scope"));
                    }
                    let package =
                        Arc::new(Package::load(&dir, &scope(root.as_deref()), &self.signature)?);
                    let receipt = self.receipt(&package)?;
                    if !receipt.enabled
                        || (!self.receipt_path(&package.identity.owner()).exists()
                            && dir.join("disabled").exists())
                    {
                        return Ok(None);
                    }
                    let permissions = package.permissions(&receipt.granted)?;
                    if !previous
                        .get(&key)
                        .is_some_and(|old| old.package.identity == package.identity)
                    {
                        super::runtime::validate(&package.wasm, &package.manifest)?;
                    }
                    Ok(Some(Loaded {
                        package,
                        permissions,
                    }))
                })();
                match result {
                    Ok(Some(entry)) => {
                        let global_conflict = root.is_some()
                            && loaded.values().any(|g: &Loaded| {
                                g.package.identity.scope == "global" && g.package.identity.id == id
                            });
                        let register = if global_conflict {
                            Err(failure("Workspace addon cannot shadow a global addon identity"))
                        } else {
                            self.install_into(&mut scoped, &entry)
                        };
                        match register {
                            Ok(()) => {
                                loaded.insert(key, entry);
                            }
                            Err(error) => {
                                errors.insert(key, error.to_string());
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        errors.insert(key, error.to_string());
                    }
                }
            }
        }
        *self.loaded.write() = loaded;
        *self.errors.write() = errors;
    }
    pub async fn rescan(&self) {
        let _maintenance = self.maintenance.lock().await;
        let Ok(_idle) = self.lifecycle.try_write() else {
            return;
        };
        self.refresh_locked();
    }
    pub async fn registry_for(
        &self,
        root: Option<&Path>,
        execution: bool,
    ) -> DaemonResult<Arc<Registry>> {
        self.registry_for_context(root, execution, &super::services::ServiceContext::default())
            .await
    }
    async fn prune_services(&self, idle: bool) {
        let identities = self
            .loaded
            .read()
            .iter()
            .map(|(k, v)| (k.clone(), v.package.identity.fingerprint.clone()))
            .collect();
        self.services.lock().await.prune(&identities, idle).await;
        self.hooks.prune(&identities);
    }
    #[cfg(test)]
    pub(crate) async fn close_services(&self, root: &Path) {
        let _maintenance = self.maintenance.lock().await;
        self.services.lock().await.close(root).await;
        self.roots.write().remove(root);
    }
    /// Admission and close share the same lock order. A pending execution owns
    /// its lease before it appears in AppState's running map.
    pub(crate) async fn close_workspace(
        &self,
        root: &Path,
        workspaces: &crate::workspace::WorkspaceManager,
    ) -> DaemonResult<()> {
        let _maintenance = self.maintenance.lock().await;
        let _idle = self.lifecycle.try_write().map_err(|_| {
            failure("Workspace close requires addon executions and chat turns to finish")
        })?;
        workspaces.close(root).await?;
        self.services.lock().await.close(root).await;
        self.roots.write().remove(root);
        self.closed_roots.write().insert(root.to_path_buf());
        self.hooks.workspace(root, false);
        Ok(())
    }
    pub(crate) fn observe_workspace_open(&self, root: &Path) {
        self.hooks.workspace(root, true);
    }
    pub(crate) async fn opened_workspace(&self, root: &Path) {
        let _maintenance = self.maintenance.lock().await;
        self.closed_roots.write().remove(root);
    }
    pub(crate) async fn registry_for_context(
        &self,
        root: Option<&Path>,
        execution: bool,
        context: &super::services::ServiceContext,
    ) -> DaemonResult<Arc<Registry>> {
        let normalized = root.map(workspace_root).transpose()?;
        let _maintenance = self.maintenance.lock().await;
        if let Some(root) = &normalized {
            if self.closed_roots.read().contains(root) {
                return Err(failure("Workspace closed before addon admission; reopen explicitly"));
            }
            self.roots.write().insert(root.clone());
        }
        self.refresh_locked();
        let idle = self.lifecycle.try_write().is_ok();
        self.prune_services(idle).await;
        let selected: Vec<_> = self
            .loaded
            .read()
            .values()
            .filter(|e| {
                e.package.identity.scope == "global"
                    || normalized
                        .as_ref()
                        .is_some_and(|r| e.package.identity.scope == scope(Some(r)))
            })
            .cloned()
            .collect();
        let mut registry = context
            .base_registry
            .as_ref()
            .map(|r| r.snapshot())
            .unwrap_or_else(|| self.registry.addon_base_snapshot());
        let mut services = self.services.lock().await;
        let mut lsp_failure =
            std::iter::once(None).chain(normalized.as_deref().map(Some)).any(|root| {
                package_dirs(&self.directory(root)).iter().any(|dir| {
                    Manifest::load(dir).is_ok_and(|(m, _)| {
                        !m.lsp.is_empty() && self.errors.read().contains_key(&owner(root, &m.id))
                    })
                })
            });
        for entry in ordered(selected) {
            let owner = entry.package.identity.owner();
            let key = super::services::key(normalized.as_deref(), &owner);
            let outcome = async {
                let mut tools = self.tools(&entry);
                let mut binding = None;
                let mut lsp = None;
                if !entry.package.manifest.mcp.is_empty() || !entry.package.manifest.lsp.is_empty()
                {
                    let instance = services
                        .admit(
                            &entry.package,
                            &entry.permissions,
                            normalized.as_deref(),
                            context,
                            &self.state_dir.join("runtime"),
                        )
                        .await?;
                    tools.extend(instance.tools.iter().cloned());
                    binding = Some(instance.binding.clone());
                    lsp = instance.lsp.clone();
                }
                // Publish every contribution together only after all services started.
                let mut candidate = registry.snapshot();
                self.install_into(&mut candidate, &entry)?;
                candidate.replace_owned_tools(&owner, tools)?;
                candidate
                    .addon_hooks
                    .extend(self.hooks.bind(entry.package.clone(), normalized.as_deref()));
                if let Some(lsp) = lsp {
                    candidate.addon_lsp.push(lsp);
                }
                if let Some(binding) = binding {
                    candidate
                        .addon_packages
                        .get_mut(&owner)
                        .expect("admitted package")
                        .registered_names
                        .push(format!("mcp-runtime:{binding}"));
                }
                Ok::<_, DaemonError>(candidate)
            }
            .await;
            match outcome {
                Ok(candidate) => {
                    registry = candidate;
                    services.errors.remove(&key);
                }
                Err(error) => {
                    lsp_failure |= !entry.package.manifest.lsp.is_empty();
                    self.hooks.retire(normalized.as_deref(), &owner);
                    if let Some(instance) = services.active.remove(&key) {
                        if idle {
                            instance.shutdown().await;
                        } else {
                            services.retired.push(instance);
                        }
                    }
                    services.errors.insert(key, error.to_string());
                }
            }
        }
        drop(services);
        self.prune_services(idle).await;
        if execution && lsp_failure {
            return Err(failure("Addon LSP admission failed; inspect addon status"));
        }
        if execution {
            registry.addon_lease =
                Some(super::services::Lease::acquire(self.lifecycle.clone(), &self.services).await);
        }
        Ok(Arc::new(registry))
    }
    pub(crate) async fn mcp_statuses(
        &self,
        root: Option<&Path>,
    ) -> Vec<crate::grpc::proto::McpServerInfo> {
        let normalized = root.and_then(|p| workspace_root(p).ok());
        let services = self.services.lock().await;
        let mut result = vec![];
        for entry in self.loaded.read().values().filter(|e| {
            e.package.identity.scope == "global"
                || normalized.as_ref().is_some_and(|r| e.package.identity.scope == scope(Some(r)))
        }) {
            let key = super::services::key(normalized.as_deref(), &entry.package.identity.owner());
            for declaration in &entry.package.manifest.mcp {
                let alias =
                    format!("{}{}", super::pascal(&entry.package.identity.id), declaration.name);
                let error = services.errors.get(&key).cloned().unwrap_or_default();
                let instance = services.active.get(&key);
                let status =
                    instance.and_then(|i| i.host.statuses().into_iter().find(|s| s.alias == alias));
                result.push(crate::grpc::proto::McpServerInfo {
                    owner: entry.package.identity.id.clone(),
                    scope_root: normalized
                        .as_ref()
                        .map(|r| r.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    name: alias.clone(),
                    status: if !error.is_empty() {
                        "Failed"
                    } else if instance.is_some_and(|i| i.overridden.contains(&alias)) {
                        "Overridden"
                    } else if status
                        .as_ref()
                        .is_some_and(|s| s.state == crate::integration::mcp::StatusKind::Connected)
                    {
                        "Connected"
                    } else {
                        "Unavailable"
                    }
                    .into(),
                    tool_count: status.map(|s| s.tool_count).unwrap_or_default(),
                    error,
                });
            }
        }
        result
    }
    pub async fn fragments_for(&self, root: &Path) -> Vec<metteur_shared::SystemFragment> {
        let root = match workspace_root(root) {
            Ok(root) => root,
            Err(_) => return vec![],
        };
        self.loaded
            .read()
            .values()
            .filter(|entry| {
                entry.package.identity.scope == "global"
                    || entry.package.identity.scope == scope(Some(&root))
            })
            .flat_map(|entry| entry.package.fragments.clone())
            .collect()
    }
    fn find_dir(&self, id: &str, root: Option<&Path>) -> DaemonResult<PathBuf> {
        let dirs: Vec<_> = package_dirs(&self.directory(root))
            .into_iter()
            .filter(|dir| {
                Manifest::load(dir).is_ok_and(|(m, _)| m.id == id)
                    || dir.file_name().is_some_and(|name| name == id)
            })
            .collect();
        if dirs.len() != 1 {
            return Err(failure("Addon is missing or its identity is ambiguous"));
        }
        Ok(dirs[0].clone())
    }
    fn preflight(&self, candidate: &Loaded, root: Option<&Path>) -> DaemonResult<()> {
        let mut registry = self.snapshot_locked(root)?;
        self.install_into(&mut registry, candidate)?;
        if root.is_none() {
            for root in self.roots.read().iter() {
                let mut scoped = registry.snapshot();
                for entry in ordered(
                    self.loaded
                        .read()
                        .values()
                        .filter(|entry| entry.package.identity.scope == scope(Some(root)))
                        .cloned()
                        .collect(),
                ) {
                    self.install_into(&mut scoped, &entry)?;
                }
            }
        }
        Ok(())
    }
    pub async fn install(
        &self,
        source: &Path,
        workspace: Option<&Path>,
        granted: &[String],
    ) -> DaemonResult<AddonInfoData> {
        let root = workspace.map(workspace_root).transpose()?;
        let _maintenance = self.maintenance.lock().await;
        let _idle = self.lifecycle.try_write().map_err(|_| {
            failure("Addon changes require active executions and chat turns to finish")
        })?;
        if let Some(root) = &root {
            if self.closed_roots.read().contains(root) {
                return Err(failure(
                    "Workspace closed before addon installation; reopen explicitly",
                ));
            }
            self.roots.write().insert(root.clone());
        }
        self.refresh_locked();
        let base = self.directory(root.as_deref());
        std::fs::create_dir_all(&base)?;
        let staging = base.join(format!(".staging-{}", uuid::Uuid::new_v4()));
        let result = (|| -> DaemonResult<AddonInfoData> {
            if source.is_dir() {
                write_snapshot(&Snapshot::read(source)?, &staging)?;
            } else if source
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
            {
                super::unzip_into(source, &staging)?;
            } else {
                return Err(failure("Addon source must be a directory or zip package"));
            }
            let mut package = Package::load(&staging, &scope(root.as_deref()), &self.signature)?;
            let permissions = package.permissions(granted)?;
            super::runtime::validate(&package.wasm, &package.manifest)?;
            let id = package.identity.id.clone();
            let matches: Vec<_> = package_dirs(&base)
                .into_iter()
                .filter(|dir| {
                    Manifest::load(dir).is_ok_and(|(m, _)| m.id == id)
                        || dir.file_name().is_some_and(|name| name == id.as_str())
                })
                .collect();
            if matches.len() > 1 {
                return Err(failure("Cannot replace ambiguous addon identity"));
            }
            let old = matches.into_iter().next();
            let dest = base.join(&id);
            if dest.exists() && old.as_ref() != Some(&dest) {
                return Err(failure("Destination belongs to a different or invalid package"));
            }
            package.path = dest.clone();
            let candidate = Loaded {
                package: Arc::new(package),
                permissions,
            };
            self.preflight(&candidate, root.as_deref())?;
            let backup = base.join(format!(".backup-{}", uuid::Uuid::new_v4()));
            if let Some(old) = &old {
                std::fs::rename(old, &backup)?;
            }
            if let Err(error) = std::fs::rename(&staging, &dest) {
                if let Some(old) = &old {
                    let _ = std::fs::rename(&backup, old);
                }
                return Err(error.into());
            }
            let receipt = Receipt {
                identity: candidate.package.identity.clone(),
                granted: granted.to_vec(),
                enabled: true,
            };
            if let Err(error) =
                write_receipt(&self.receipt_path(&receipt.identity.owner()), &receipt)
            {
                let _ = std::fs::rename(&dest, &staging);
                if let Some(old) = &old {
                    let _ = std::fs::rename(&backup, old);
                }
                return Err(error);
            }
            if backup.exists() {
                remove_owned_directory(&base, &backup)?;
            }
            self.refresh_locked();
            self.hooks.retire_owner(&candidate.package.identity.owner());
            self.info(&id, root.as_deref())
        })();
        if staging.exists() {
            let _ = remove_owned_directory(&base, &staging);
        }
        self.prune_services(true).await;
        result
    }
    pub async fn uninstall(&self, id: &str, workspace: Option<&Path>) -> DaemonResult<()> {
        let root = workspace.map(workspace_root).transpose()?;
        let _maintenance = self.maintenance.lock().await;
        let _idle = self.lifecycle.try_write().map_err(|_| {
            failure("Addon changes require active executions and chat turns to finish")
        })?;
        let dir = self.find_dir(id, root.as_deref())?;
        remove_owned_directory(&self.directory(root.as_deref()), &dir)?;
        let receipt = self.receipt_path(&owner(root.as_deref(), id));
        if receipt.exists() {
            std::fs::remove_file(receipt)?;
        }
        self.hooks.retire_owner(&owner(root.as_deref(), id));
        self.refresh_locked();
        self.prune_services(true).await;
        Ok(())
    }
    pub async fn set_enabled(
        &self,
        id: &str,
        enabled: bool,
        workspace: Option<&Path>,
    ) -> DaemonResult<()> {
        let root = workspace.map(workspace_root).transpose()?;
        let _maintenance = self.maintenance.lock().await;
        let _idle = self.lifecycle.try_write().map_err(|_| {
            failure("Addon changes require active executions and chat turns to finish")
        })?;
        let dir = self.find_dir(id, root.as_deref())?;
        let package = Package::load(&dir, &scope(root.as_deref()), &self.signature)?;
        let mut receipt = self.receipt(&package)?;
        if enabled {
            if root.as_ref().is_some_and(|root| self.closed_roots.read().contains(root)) {
                return Err(failure("Workspace closed before addon activation; reopen explicitly"));
            }
            package.permissions(&receipt.granted)?;
            super::runtime::validate(&package.wasm, &package.manifest)?;
        }
        receipt.enabled = enabled;
        write_receipt(&self.receipt_path(&receipt.identity.owner()), &receipt)?;
        self.hooks.retire_owner(&receipt.identity.owner());
        self.refresh_locked();
        self.prune_services(true).await;
        Ok(())
    }
    fn info(&self, id: &str, root: Option<&Path>) -> DaemonResult<AddonInfoData> {
        let dir = self.find_dir(id, root)?;
        let (manifest, _) = Manifest::load(&dir)?;
        let key = owner(root, id);
        let loaded = self.loaded.read();
        let entry = loaded.get(&key);
        let error = self.errors.read().get(&key).cloned().unwrap_or_default();
        let package = Package::load(&dir, &scope(root), &self.signature).ok();
        let receipt = package.as_ref().and_then(|p| self.receipt(p).ok());
        Ok(AddonInfoData {
            id: manifest.id,
            version: manifest.version,
            name: manifest.name,
            description: manifest.description,
            enabled: entry.is_some(),
            scope: if root.is_some() {
                "workspace"
            } else {
                "global"
            }
            .into(),
            scope_root: root.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
            fingerprint: package
                .as_ref()
                .map(|p| p.identity.fingerprint.clone())
                .unwrap_or_default(),
            status: if !error.is_empty() {
                "Failed"
            } else if entry.is_some() {
                "Loaded"
            } else {
                "Disabled"
            }
            .into(),
            error,
            required_permissions: manifest.permissions.required,
            granted_permissions: receipt.map(|r| r.granted).unwrap_or_default(),
            tool_count: manifest.tools.len() as u32,
            fragment_count: manifest.fragments.len() as u32,
            hooks: vec![],
        })
    }
    pub async fn list(&self, roots: &[PathBuf]) -> Vec<AddonInfoData> {
        let _maintenance = self.maintenance.lock().await;
        let roots: Vec<_> = roots.iter().filter_map(|root| workspace_root(root).ok()).collect();
        self.roots.write().extend(roots.iter().cloned());
        self.refresh_locked();
        let selected_roots = roots.clone();
        self.prune_services(self.lifecycle.try_write().is_ok()).await;
        let roots: Vec<_> = std::iter::once(None).chain(roots.iter().cloned().map(Some)).collect();
        let services = self.services.lock().await;
        let mut result = vec![];
        for root in roots {
            for dir in package_dirs(&self.directory(root.as_deref())) {
                let id = Manifest::load(&dir).map(|(m, _)| m.id).unwrap_or_else(|_| {
                    dir.file_name().unwrap_or_default().to_string_lossy().into_owned()
                });
                match self.info(&id, root.as_deref()) {
                    Ok(mut info) => {
                        let key = owner(root.as_deref(), &info.id);
                        info.hooks = self.hooks.reports(&key, &info.fingerprint, &selected_roots);
                        let relevant: Vec<_> =
                            std::iter::once(super::services::key(root.as_deref(), &key))
                                .chain(
                                    selected_roots
                                        .iter()
                                        .map(|r| super::services::key(Some(r), &key)),
                                )
                                .collect();
                        if let Some(error) = relevant.iter().find_map(|k| services.errors.get(k)) {
                            info.status = "Failed".into();
                            info.error = error.clone();
                            info.enabled = false;
                            info.tool_count = 0;
                            info.fragment_count = 0;
                        } else if let Some(instance) =
                            relevant.iter().find_map(|k| services.active.get(k))
                        {
                            info.tool_count += instance.tools.len() as u32;
                            for service in relevant.iter().filter_map(|k| services.active.get(k)) {
                                if let Some(lsp) = &service.lsp
                                    && !lsp.healthy().await
                                {
                                    info.status = "Failed".into();
                                    info.error = "Addon LSP connection failed; disable and enable the package to retry".into();
                                    info.enabled = false;
                                }
                            }
                        }
                        result.push(info)
                    }
                    Err(_) => result.push(AddonInfoData {
                        id,
                        version: String::new(),
                        name: "Unavailable package".into(),
                        description: String::new(),
                        enabled: false,
                        scope: if root.is_some() {
                            "workspace"
                        } else {
                            "global"
                        }
                        .into(),
                        scope_root: root
                            .as_ref()
                            .map(|r| r.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        fingerprint: String::new(),
                        status: "Failed".into(),
                        error: "Invalid or duplicate package; inspect installation".into(),
                        required_permissions: vec![],
                        granted_permissions: vec![],
                        tool_count: 0,
                        fragment_count: 0,
                        hooks: vec![],
                    }),
                }
            }
        }
        result
    }
}

fn package_dirs(base: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<_> = std::fs::read_dir(base)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    dirs
}
fn write_snapshot(snapshot: &Snapshot, dest: &Path) -> DaemonResult<()> {
    std::fs::create_dir_all(dest)?;
    for (name, bytes) in &snapshot.files {
        super::manifest::package_file(&snapshot.files, name)?;
        let path = dest.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, bytes)?;
    }
    Ok(())
}
fn remove_owned_directory(base: &Path, path: &Path) -> DaemonResult<()> {
    let base = base.canonicalize()?;
    let resolved = path.canonicalize()?;
    if resolved == base
        || !resolved.starts_with(&base)
        || std::fs::symlink_metadata(path)?.file_type().is_symlink()
    {
        return Err(failure("Refusing addon cleanup outside its owning directory"));
    }
    std::fs::remove_dir_all(path)?;
    Ok(())
}
fn write_receipt(path: &Path, receipt: &Receipt) -> DaemonResult<()> {
    let parent = path.parent().ok_or_else(|| failure("Missing addon receipt directory"))?;
    std::fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(".receipt-{}", uuid::Uuid::new_v4()));
    let backup = parent.join(format!(".receipt-backup-{}", uuid::Uuid::new_v4()));
    let bytes =
        toml::to_string(receipt).map_err(|_| failure("Cannot serialize addon authorization"))?;
    {
        use std::io::Write;
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes.as_bytes())?;
        file.sync_all()?;
    }
    let existed = path.exists();
    if existed {
        std::fs::rename(path, &backup)?;
    }
    if let Err(error) = std::fs::rename(&tmp, path) {
        if existed {
            let _ = std::fs::rename(&backup, path);
        }
        let _ = std::fs::remove_file(tmp);
        return Err(error.into());
    }
    if existed {
        std::fs::remove_file(backup)?;
    }
    Ok(())
}
