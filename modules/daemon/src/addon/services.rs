//! Owned MCP contributions use the existing MCP host and scoped authorization.
use super::{package::Package, runtime::Permission};
use crate::{
    DaemonError, DaemonResult,
    execution::context::ExecutionContext,
    integration::mcp::{
        McpHost,
        connection::{GuardedConnection, RmcpConnection},
    },
    observability::metrics::Metrics,
    registry::{Registry, Tool},
    storage::persistence::Db,
};
use metteur_shared::config::McpConfig;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone, Default)]
pub(crate) struct ServiceContext {
    pub base_registry: Option<Arc<Registry>>,
    pub config: McpConfig,
    pub lsp_config: metteur_shared::config::LspConfig,
    pub lsp_disabled: bool,
    pub workspace_db: Option<Db>,
    pub global_db: Option<Db>,
    pub metrics: Arc<Metrics>,
}
pub(super) struct Instance {
    pub owner: String,
    pub fingerprint: String,
    pub scope: String,
    pub host: Arc<McpHost>,
    pub tools: Vec<Arc<dyn Tool>>,
    pub overridden: Vec<String>,
    pub binding: String,
    pub lsp: Option<Arc<crate::integration::lsp::LspManager>>,
    config_binding: String,
    timeout: u64,
    runtime: PathBuf,
}
impl Instance {
    pub async fn shutdown(&self) {
        self.host.shutdown().await;
        if let Some(lsp) = &self.lsp {
            lsp.shutdown().await;
        }
    }
}
impl Drop for Instance {
    fn drop(&mut self) {
        // This UUID directory was created by this instance, never a package path.
        let _ = std::fs::remove_dir_all(&self.runtime);
    }
}
#[derive(Default)]
pub(super) struct Services {
    pub active: BTreeMap<String, Arc<Instance>>,
    pub retired: Vec<Arc<Instance>>,
    pub errors: BTreeMap<String, String>,
}

/// The last execution snapshot releases retired connections without requiring
/// another discovery request or a management operation.
pub(crate) struct Lease {
    guard: Option<tokio::sync::OwnedRwLockReadGuard<()>>,
    lifecycle: Arc<tokio::sync::RwLock<()>>,
    services: std::sync::Weak<tokio::sync::Mutex<Services>>,
}
impl Lease {
    pub(super) async fn acquire(
        lifecycle: Arc<tokio::sync::RwLock<()>>,
        services: &Arc<tokio::sync::Mutex<Services>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            guard: Some(lifecycle.clone().read_owned().await),
            lifecycle,
            services: Arc::downgrade(services),
        })
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        drop(self.guard.take());
        let Some(services) = self.services.upgrade() else {
            return;
        };
        if services.try_lock().is_ok_and(|state| state.retired.is_empty()) {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let lifecycle = self.lifecycle.clone();
        runtime.spawn(async move {
            let Ok(_idle) = lifecycle.try_write_owned() else {
                return;
            };
            services.lock().await.drain_retired().await;
        });
    }
}
fn error(message: &str) -> DaemonError {
    DaemonError::Addon(message.into())
}
pub(super) fn key(root: Option<&Path>, owner: &str) -> String {
    format!("{}|{owner}", root.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default())
}

impl Services {
    async fn drain_retired(&mut self) {
        for instance in self.retired.drain(..) {
            instance.shutdown().await;
        }
    }
    pub async fn prune(&mut self, identities: &BTreeMap<String, String>, idle: bool) {
        self.errors
            .retain(|key, _| identities.keys().any(|owner| key.ends_with(&format!("|{owner}"))));
        let stale: Vec<_> = self
            .active
            .iter()
            .filter(|(_, v)| identities.get(&v.owner) != Some(&v.fingerprint))
            .map(|(k, _)| k.clone())
            .collect();
        for key in stale {
            if let Some(instance) = self.active.remove(&key) {
                self.retired.push(instance);
            }
        }
        if idle {
            self.drain_retired().await;
        }
    }
    pub async fn close(&mut self, root: &Path) {
        let scope = root.to_string_lossy();
        let stale: Vec<_> =
            self.active.iter().filter(|(_, v)| v.scope == scope).map(|(k, _)| k.clone()).collect();
        for key in stale {
            if let Some(instance) = self.active.remove(&key) {
                instance.shutdown().await;
            }
        }
        let retired = std::mem::take(&mut self.retired);
        for instance in retired {
            if instance.scope == scope {
                instance.shutdown().await;
            } else {
                self.retired.push(instance);
            }
        }
    }
    pub async fn admit(
        &mut self,
        package: &Package,
        permissions: &HashSet<Permission>,
        root: Option<&Path>,
        context: &ServiceContext,
        base: &Path,
    ) -> DaemonResult<Arc<Instance>> {
        let key = key(root, &package.identity.owner());
        let signature = package.identity.fingerprint.clone();
        if let Some(root) = root {
            super::lsp::authorize(package, permissions, root, context).await?;
        }
        // Recheck persistent denies even when reusing a live session.
        for entry in &package.manifest.mcp {
            let alias = format!("{}{}", super::pascal(&package.identity.id), entry.name);
            if !context.config.servers.contains_key(&alias) && entry.server.transport == "stdio" {
                authorize(&entry.server.command, permissions, root.unwrap_or(base), context)
                    .await?;
            }
        }
        // Configuration changes retire the previous session before re-admission.
        let overridden: Vec<_> = package
            .manifest
            .mcp
            .iter()
            .map(|e| format!("{}{}", super::pascal(&package.identity.id), e.name))
            .filter(|a| context.config.servers.contains_key(a))
            .collect();
        use sha2::{Digest, Sha256};
        let mut config_hash = Sha256::new();
        if !package.manifest.lsp.is_empty() {
            config_hash.update([u8::from(context.lsp_disabled)]);
            config_hash.update(
                serde_json::to_vec(&context.lsp_config)
                    .map_err(|_| error("Invalid LSP configuration"))?,
            );
        }
        for alias in &overridden {
            config_hash.update(alias.as_bytes());
            config_hash.update(format!("{:?}", context.config.servers[alias]).as_bytes());
        }
        let config_binding: String =
            config_hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
        let timeout = if context.config.call_timeout_secs == 0 {
            30
        } else {
            context.config.call_timeout_secs.clamp(1, 30)
        };
        if let Some(instance) = self.active.get(&key)
            && instance.fingerprint == signature
            && instance.overridden == overridden
            && instance.timeout == timeout
            && instance.config_binding == config_binding
        {
            if let Some(lsp) = &instance.lsp
                && !lsp.healthy().await
            {
                return Err(error("Addon LSP connection failed"));
            }
            if instance
                .host
                .statuses()
                .iter()
                .any(|s| s.state == crate::integration::mcp::StatusKind::Failed)
            {
                return Err(error("Addon MCP connection failed"));
            }
            return Ok(instance.clone());
        }
        if let Some(old) = self.active.remove(&key) {
            self.retired.push(old);
        }
        std::fs::create_dir_all(base)?;
        let runtime = base.join(format!("mcp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&runtime)?;
        let host = McpHost::owned(context.metrics.clone());
        let mut instance = Instance {
            owner: package.identity.owner(),
            fingerprint: signature,
            scope: root.map(|r| r.to_string_lossy().into_owned()).unwrap_or_default(),
            host,
            tools: vec![],
            overridden,
            binding: config_binding.clone(),
            config_binding,
            timeout,
            runtime,
            lsp: None,
        };
        let outcome = async {
            for (name, bytes) in package.files.iter() {
                super::manifest::package_file(&package.files, name)?;
                let dest = instance.runtime.join(name);
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(dest, bytes)?;
            }
            if let Some(root) = root {
                instance.lsp =
                    super::lsp::start(package, permissions, root, context, &instance.runtime)
                        .await?;
            }
            for entry in &package.manifest.mcp {
                let alias = format!("{}{}", super::pascal(&package.identity.id), entry.name);
                if instance.overridden.contains(&alias) {
                    continue;
                }
                let mut secrets = Vec::new();
                let declaration = &entry.server;
                let resolve = |name: &str| -> DaemonResult<String> {
                    if !permissions.contains(&Permission::Environment) {
                        return Err(error("Addon environment grant required"));
                    }
                    let value = std::env::var(name)
                        .map_err(|_| error("Addon credential reference unavailable"))?;
                    if value.len() > 16384 {
                        return Err(error("Addon credential reference exceeds limit"));
                    }
                    Ok(value)
                };
                let connecting = async {
                    if declaration.transport == "stdio" {
                        let command: Vec<_> = declaration
                            .command
                            .iter()
                            .map(|arg| {
                                arg.strip_prefix("${package}/")
                                    .map(|p| {
                                        instance.runtime.join(p).to_string_lossy().into_owned()
                                    })
                                    .unwrap_or_else(|| arg.clone())
                            })
                            .collect();
                        #[cfg(unix)]
                        if declaration.command[0].starts_with("${package}/") {
                            use std::os::unix::fs::PermissionsExt;
                            std::fs::set_permissions(
                                &command[0],
                                std::fs::Permissions::from_mode(0o700),
                            )?;
                        }
                        let mut env = HashMap::new();
                        for (name, reference) in &declaration.env_refs {
                            let value = resolve(reference)?;
                            secrets.push(value.clone());
                            env.insert(name.clone(), value);
                        }
                        RmcpConnection::connect_stdio_owned(
                            &command,
                            &env,
                            root.unwrap_or(&instance.runtime),
                        )
                        .await
                    } else {
                        if !permissions.contains(&Permission::Network) {
                            return Err(error("Addon network grant required"));
                        }
                        let authorization =
                            declaration.auth_env.as_ref().map(|name| resolve(name)).transpose()?;
                        if let Some(value) = &authorization {
                            secrets.push(value.clone());
                        }
                        RmcpConnection::connect_http_owned(
                            declaration
                                .url
                                .as_deref()
                                .ok_or_else(|| error("Addon MCP endpoint missing"))?,
                            authorization.as_deref(),
                        )
                        .await
                    }
                };
                let inner =
                    tokio::time::timeout(std::time::Duration::from_secs(timeout), connecting)
                        .await
                        .map_err(|_| error("Addon MCP initialization timed out"))??;
                let tools = instance
                    .host
                    .adopt(
                        &alias,
                        Arc::new(GuardedConnection {
                            inner,
                            secrets,
                        }),
                        timeout,
                    )
                    .await?;
                instance.tools.extend(tools);
            }
            if !instance.host.statuses().is_empty() {
                let prefix = super::pascal(&package.identity.id);
                instance.tools.push(Arc::new(NamedTool {
                    name: format!("{prefix}ListMcpResources"),
                    inner: Arc::new(crate::integration::mcp::tools::ListMcpResources::new(
                        instance.host.clone(),
                    )),
                }));
                instance.tools.push(Arc::new(NamedTool {
                    name: format!("{prefix}ReadMcpResource"),
                    inner: Arc::new(crate::integration::mcp::tools::ReadMcpResource::new(
                        instance.host.clone(),
                    )),
                }));
            }
            Ok::<(), DaemonError>(())
        }
        .await;
        if let Err(error) = outcome {
            instance.shutdown().await;
            return Err(error);
        }
        let mut hash = Sha256::new();
        hash.update(instance.binding.as_bytes());
        for tool in &instance.tools {
            hash.update(tool.name().as_bytes());
            hash.update(tool.parameters().to_string().as_bytes());
        }
        instance.binding = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
        let instance = Arc::new(instance);
        self.active.insert(key, instance.clone());
        Ok(instance)
    }
}
struct NamedTool {
    name: String,
    inner: Arc<dyn Tool>,
}
#[async_trait::async_trait]
impl Tool for NamedTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        self.inner.description()
    }
    fn parameters(&self) -> serde_json::Value {
        self.inner.parameters()
    }
    async fn call(
        &self,
        args: &[metteur_shared::Value],
        ctx: &mut ExecutionContext,
    ) -> DaemonResult<metteur_shared::Value> {
        self.inner.call(args, ctx).await
    }
}
pub(super) async fn authorize(
    command: &[String],
    permissions: &HashSet<Permission>,
    root: &Path,
    context: &ServiceContext,
) -> DaemonResult<()> {
    if !permissions.contains(&Permission::Process) {
        return Err(error("Addon process grant required"));
    }
    let mut ctx = ExecutionContext::new(
        Arc::new(Registry::default()),
        crate::llm::LlmClientFactory::new(),
        root.to_path_buf(),
    );
    ctx.workspace_db = context.workspace_db.clone();
    ctx.global_db = context.global_db.clone();
    ctx.permission_mode = crate::sandbox::PermissionMode::Ask;
    let broker = Arc::new(crate::sandbox::approval::ApprovalBroker::new());
    let command = command.join(" ");
    broker.record_run_grant(
        crate::sandbox::command_hash(&crate::sandbox::policy::normalize(&command)),
        true,
    );
    ctx.approvals = Some(broker);
    if !crate::sandbox::authorize(&ctx, &command).await? {
        return Err(error("Addon service startup denied by sandbox"));
    }
    Ok(())
}
