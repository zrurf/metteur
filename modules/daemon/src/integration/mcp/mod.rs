//! MCP host: connection lifecycle, registry integration and status.
//!
//! The host is daemon-global and driven by the global `[mcp]` config section;
//! it re-syncs on every config broadcast (startup and `SetConfig`).

pub mod connection;
pub mod tools;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use metteur_shared::config::{McpConfig, McpServerConfig};
use parking_lot::RwLock;
use tokio::sync::Mutex as AsyncMutex;

use crate::error::{DaemonError, DaemonResult};
use crate::observability::metrics::Metrics;
use crate::registry::Registry;

use connection::McpConnectionOps;

/// Merges the global MCP servers with those declared by open workspaces.
///
/// Server processes are daemon-wide, so the effective set is a union: a
/// workspace declaration overrides a global one with the same alias (the more
/// specific scope wins), and servers only some workspace declares stay
/// available while that workspace is open. The section-level timeout keeps the
/// global value, since one host carries a single timeout.
pub fn merge_servers(global: &McpConfig, workspace_configs: &[McpConfig]) -> McpConfig {
    let mut merged = global.clone();
    for config in workspace_configs {
        for (alias, server) in &config.servers {
            merged.servers.insert(alias.clone(), server.clone());
        }
    }
    merged
}

/// Default per-call timeout when `[mcp] call_timeout_secs` is unset.
pub(crate) const DEFAULT_CALL_TIMEOUT_SECS: u64 = 30;

/// The observed state of one configured server.
#[derive(Debug, Clone)]
enum ServerState {
    /// Connected with the given number of discovered tools.
    Connected(usize),
    /// Disabled in configuration; no connection attempt is made.
    Disabled,
    /// Connection or discovery failed.
    Failed(String),
}

struct ServerEntry {
    state: ServerState,
    conn: Option<Arc<dyn McpConnectionOps>>,
    /// Registry names of tools registered for this server.
    registered_tools: Vec<String>,
    /// Fingerprint of the config that produced the current connection.
    fingerprint: String,
    /// Resolved per-call timeout in seconds.
    timeout_secs: u64,
}

/// Manages MCP server connections and their registry tools.
pub struct McpHost {
    registry: Arc<Registry>,
    metrics: Arc<Metrics>,
    servers: RwLock<HashMap<String, ServerEntry>>,
    /// Serializes tool calls so remote sessions stay orderly.
    call_lock: AsyncMutex<()>,
}

impl McpHost {
    /// An addon owns this host; adapters are published atomically by AddonHost.
    pub(crate) fn owned(metrics: Arc<Metrics>) -> Arc<Self> {
        Arc::new(Self {
            registry: Arc::new(Registry::default()),
            metrics,
            servers: Default::default(),
            call_lock: AsyncMutex::new(()),
        })
    }

    pub(crate) async fn adopt(
        self: &Arc<Self>,
        alias: &str,
        conn: Arc<dyn McpConnectionOps>,
        timeout: u64,
    ) -> DaemonResult<Vec<Arc<dyn crate::registry::Tool>>> {
        let discovered =
            tokio::time::timeout(Duration::from_secs(timeout), conn.list_tools()).await;
        let result = (|| {
            let remotes = discovered
                .map_err(|_| DaemonError::Mcp("Addon MCP discovery timed out".into()))??;
            if remotes.len() > 256 {
                return Err(DaemonError::Mcp("Addon MCP tool limit exceeded".into()));
            }
            let mut names = HashSet::new();
            let mut adapters = Vec::<Arc<dyn crate::registry::Tool>>::new();
            for remote in remotes {
                if remote.name.len() > 128
                    || remote.description.len() > 16384
                    || remote.schema.to_string().len() > 65536
                {
                    return Err(DaemonError::Mcp("Addon MCP discovery limit exceeded".into()));
                }
                let (name, _) =
                    tools::unique_server_tool_name(alias, &remote.name, &HashSet::new());
                if !names.insert(name.clone()) {
                    return Err(DaemonError::Mcp("Addon MCP tool name collision".into()));
                }
                adapters.push(Arc::new(tools::McpServerTool::new(
                    self.clone(),
                    alias,
                    remote,
                    name,
                )));
            }
            Ok(adapters)
        })();
        match result {
            Ok(adapters) => {
                self.servers.write().insert(
                    alias.into(),
                    ServerEntry {
                        state: ServerState::Connected(adapters.len()),
                        conn: Some(conn),
                        registered_tools: vec![],
                        fingerprint: String::new(),
                        timeout_secs: timeout,
                    },
                );
                Ok(adapters)
            }
            Err(error) => {
                conn.shutdown().await;
                Err(error)
            }
        }
    }
    pub(crate) async fn shutdown(&self) {
        let aliases: Vec<_> = self.servers.read().keys().cloned().collect();
        for alias in aliases {
            self.teardown_server(&alias).await;
        }
    }
    /// Creates a host attached to the shared registry and metrics.
    ///
    /// The host-level resource tools (`ListMcpResources`, `ReadMcpResource`)
    /// are registered here; they take the server alias as an argument, so one
    /// copy serves every connected server.
    pub fn new(registry: Arc<Registry>, metrics: Arc<Metrics>) -> Arc<Self> {
        let host = Arc::new(Self {
            registry: registry.clone(),
            metrics,
            servers: RwLock::new(HashMap::new()),
            call_lock: AsyncMutex::new(()),
        });
        for tool in [
            Arc::new(tools::ListMcpResources::new(host.clone())) as Arc<dyn crate::registry::Tool>,
            Arc::new(tools::ReadMcpResource::new(host.clone())),
        ] {
            registry.try_register_tool(tool).expect("MCP resource tool names are valid");
        }
        host
    }

    /// Aligns live connections with the configured set of servers.
    ///
    /// New or changed servers are connected and their tools registered;
    /// removed or disabled servers are shut down and unregistered. Failures
    /// are isolated per server and recorded in its status.
    pub async fn sync(self: &Arc<Self>, config: &McpConfig) {
        let section_timeout = if config.call_timeout_secs == 0 {
            DEFAULT_CALL_TIMEOUT_SECS
        } else {
            config.call_timeout_secs
        };
        let mut desired: HashSet<String> = HashSet::new();

        for (alias, server_config) in &config.servers {
            desired.insert(alias.clone());
            let fingerprint = fingerprint(server_config);
            let unchanged = {
                self.servers.read().get(alias).is_some_and(|entry| {
                    entry.fingerprint == fingerprint
                        && matches!(entry.state, ServerState::Connected(_))
                })
            };
            if unchanged {
                // Section-level timeout changes must reach already-connected
                // servers without forcing a reconnect.
                if let Some(entry) = self.servers.write().get_mut(alias) {
                    entry.timeout_secs = section_timeout;
                }
                continue;
            }
            // Reconnect disabled or changed servers from scratch; also record
            // newly-disabled entries so they show up in status output.
            self.teardown_server(alias).await;
            if !server_config.enabled {
                self.servers.write().insert(
                    alias.clone(),
                    ServerEntry {
                        state: ServerState::Disabled,
                        conn: None,
                        registered_tools: Vec::new(),
                        fingerprint,
                        timeout_secs: section_timeout,
                    },
                );
                continue;
            }
            match self
                .connect_and_register(alias, server_config, fingerprint.clone(), section_timeout)
                .await
            {
                Ok(count) => {
                    tracing::info!("MCP server '{alias}' connected with {count} tool(s)");
                }
                Err(err) => {
                    tracing::warn!("MCP server '{alias}' failed: {err}");
                    self.servers.write().insert(
                        alias.clone(),
                        ServerEntry {
                            state: ServerState::Failed(err.to_string()),
                            conn: None,
                            registered_tools: Vec::new(),
                            fingerprint,
                            timeout_secs: section_timeout,
                        },
                    );
                }
            }
        }

        // Shut down servers that disappeared from the config entirely.
        let stale: Vec<String> = {
            let servers = self.servers.read();
            servers.keys().filter(|alias| !desired.contains(*alias)).cloned().collect()
        };
        for alias in stale {
            self.teardown_server(&alias).await;
        }
    }

    /// Returns the status snapshot of every known server.
    pub fn statuses(&self) -> Vec<ServerStatus> {
        self.servers
            .read()
            .iter()
            .map(|(alias, entry)| {
                let (state, tool_count, error) = match &entry.state {
                    ServerState::Connected(count) => {
                        (StatusKind::Connected, *count as u32, String::new())
                    }
                    ServerState::Disabled => (StatusKind::Disabled, 0, String::new()),
                    ServerState::Failed(message) => (StatusKind::Failed, 0, message.clone()),
                };
                ServerStatus {
                    alias: alias.clone(),
                    state,
                    tool_count,
                    error,
                }
            })
            .collect()
    }

    /// Calls a remote tool by server alias and wire name.
    pub async fn call_server_tool(
        &self,
        alias: &str,
        remote_name: &str,
        args: serde_json::Map<String, serde_json::Value>,
    ) -> DaemonResult<String> {
        let (conn, timeout) = {
            let servers = self.servers.read();
            let entry = servers.get(alias).ok_or_else(|| {
                DaemonError::NotFound(format!("MCP server '{alias}' is not connected"))
            })?;
            if matches!(entry.state, ServerState::Failed(_)) {
                return Err(DaemonError::Mcp(
                    "MCP connection failed; refresh service status".into(),
                ));
            }
            let conn = entry.conn.clone().ok_or_else(|| {
                DaemonError::Mcp(format!("server '{alias}' has no active connection"))
            })?;
            (conn, entry.timeout_secs)
        };

        let _guard = self.call_lock.lock().await;
        let result = tokio::time::timeout(
            Duration::from_secs(timeout),
            conn.call_tool(remote_name, serde_json::Value::Object(args)),
        )
        .await
        .map_err(|_| DaemonError::Mcp(format!("tool {remote_name} timed out after {timeout}s")))
        .and_then(|r| {
            r.map_err(|err| DaemonError::Mcp(format!("tool {remote_name} failed: {err}")))
        });
        if result.is_err() {
            let mut servers = self.servers.write();
            if let Some(entry) = servers.get_mut(alias)
                && entry.fingerprint.is_empty()
            {
                entry.state =
                    ServerState::Failed("Addon MCP request failed or connection closed".into());
            }
        }
        self.metrics.record_mcp_call(alias);
        result
    }

    /// Lists resources exposed by the given server.
    pub async fn list_server_resources(
        &self,
        alias: &str,
    ) -> DaemonResult<Vec<connection::RemoteResource>> {
        let (conn, timeout) = self.connection_with_timeout(alias)?;
        tokio::time::timeout(Duration::from_secs(timeout), conn.list_resources())
            .await
            .map_err(|_| DaemonError::Mcp(format!("listing resources timed out after {timeout}s")))?
            .map_err(|err| DaemonError::Mcp(err.to_string()))
    }

    /// Reads one resource as text from the given server.
    pub async fn read_server_resource(&self, alias: &str, uri: &str) -> DaemonResult<String> {
        let (conn, timeout) = self.connection_with_timeout(alias)?;
        tokio::time::timeout(Duration::from_secs(timeout), conn.read_resource_text(uri))
            .await
            .map_err(|_| DaemonError::Mcp(format!("reading resource timed out after {timeout}s")))?
            .map_err(|err| DaemonError::Mcp(err.to_string()))
    }

    async fn connect_and_register(
        self: &Arc<Self>,
        alias: &str,
        server_config: &McpServerConfig,
        fingerprint: String,
        timeout_secs: u64,
    ) -> DaemonResult<usize> {
        let conn: Arc<dyn McpConnectionOps> = if server_config.transport == "http" {
            let url = server_config.url.clone().ok_or_else(|| {
                DaemonError::Mcp(format!("server '{alias}' uses http transport without url"))
            })?;
            Arc::new(connection::RmcpConnection::connect_http(&url).await?)
        } else {
            Arc::new(
                connection::RmcpConnection::connect_stdio(
                    &server_config.command,
                    &server_config.env,
                )
                .await?,
            )
        };

        // Discovery failure must not leak the live connection (and, for
        // stdio, the spawned server process).
        let remote_tools = match tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            conn.list_tools(),
        )
        .await
        {
            Ok(Ok(tools)) => tools,
            Ok(Err(err)) => {
                conn.shutdown().await;
                return Err(DaemonError::Mcp(err.to_string()));
            }
            Err(_) => {
                conn.shutdown().await;
                return Err(DaemonError::Mcp("tool discovery timed out".to_string()));
            }
        };

        // Register adapters under unique names; `taken` covers everything in
        // the registry so conflicts also avoid builtin and addon names.
        let mut taken: HashSet<String> = self.registry.tool_names().into_iter().collect();
        let mut registered = Vec::with_capacity(remote_tools.len());
        for remote in &remote_tools {
            let (name, _) = tools::unique_server_tool_name(alias, &remote.name, &taken);
            taken.insert(name.clone());
            self.registry.try_register_tool(Arc::new(tools::McpServerTool::new(
                self.clone(),
                alias,
                remote.clone(),
                name.clone(),
            )))?;
            registered.push(name);
        }

        let count = registered.len();
        self.servers.write().insert(
            alias.to_string(),
            ServerEntry {
                state: ServerState::Connected(count),
                conn: Some(conn),
                registered_tools: registered,
                fingerprint,
                timeout_secs,
            },
        );
        Ok(count)
    }

    /// Unregisters this server's tools and shuts its connection down.
    async fn teardown_server(&self, alias: &str) {
        let entry = self.servers.write().remove(alias);
        if let Some(entry) = entry {
            for name in &entry.registered_tools {
                self.registry.unregister_tool(name);
            }
            if let Some(conn) = entry.conn {
                conn.shutdown().await;
            }
        }
    }

    fn connection_with_timeout(
        &self,
        alias: &str,
    ) -> DaemonResult<(Arc<dyn McpConnectionOps>, u64)> {
        self.servers
            .read()
            .get(alias)
            .and_then(|entry| entry.conn.clone().map(|conn| (conn, entry.timeout_secs)))
            .ok_or_else(|| DaemonError::Mcp(format!("server '{alias}' is not connected")))
    }
}

/// Stable string describing identity-relevant server configuration.
fn fingerprint(config: &McpServerConfig) -> String {
    format!(
        "{}/{:?}/{:?}/{:?}/{}",
        config.transport, config.command, config.env, config.url, config.enabled
    )
}

/// Public status snapshot for `ListMcpServers`.
#[derive(Debug, Clone)]
pub struct ServerStatus {
    pub alias: String,
    pub state: StatusKind,
    pub tool_count: u32,
    pub error: String,
}

/// Coarse connection state kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    Connected,
    Disabled,
    Failed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_registers_resource_tools() {
        let registry = Arc::new(Registry::default());
        let _host = McpHost::new(registry.clone(), Arc::new(Metrics::default()));
        assert!(registry.tool("ListMcpResources").is_some());
        assert!(registry.tool("ReadMcpResource").is_some());
    }
}

#[cfg(test)]
mod merge_tests {
    use super::*;

    fn server(command: &str) -> McpServerConfig {
        McpServerConfig {
            transport: "stdio".to_string(),
            command: vec![command.to_string()],
            enabled: true,
            ..Default::default()
        }
    }

    fn config(servers: &[(&str, &str)]) -> McpConfig {
        McpConfig {
            servers: servers.iter().map(|(a, c)| (a.to_string(), server(c))).collect(),
            call_timeout_secs: 0,
        }
    }

    #[test]
    fn union_keeps_global_and_workspace_servers() {
        let global = config(&[("files", "filesrv")]);
        let workspace = config(&[("db", "dbsrv")]);
        let merged = merge_servers(&global, &[workspace]);
        assert_eq!(merged.servers.len(), 2);
        assert!(merged.servers.contains_key("files"));
        assert!(merged.servers.contains_key("db"));
    }

    #[test]
    fn workspace_overrides_a_global_alias() {
        let global = config(&[("shared", "global-srv")]);
        let workspace = config(&[("shared", "workspace-srv")]);
        let merged = merge_servers(&global, &[workspace]);
        assert_eq!(merged.servers.len(), 1);
        assert_eq!(merged.servers["shared"].command, vec!["workspace-srv".to_string()]);
    }

    #[test]
    fn last_workspace_wins_on_a_collision() {
        let global = McpConfig::default();
        let first = config(&[("shared", "first-srv")]);
        let second = config(&[("shared", "second-srv")]);
        let merged = merge_servers(&global, &[first, second]);
        assert_eq!(merged.servers["shared"].command, vec!["second-srv".to_string()]);
    }

    #[test]
    fn no_workspace_yields_the_global_set() {
        let global = config(&[("files", "filesrv")]);
        let merged = merge_servers(&global, &[]);
        assert_eq!(merged.servers.len(), 1);
        // The section timeout always comes from the global layer.
        assert_eq!(merged.call_timeout_secs, global.call_timeout_secs);
    }
}
