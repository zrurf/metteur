//! MCP server connections backed by the rmcp SDK.

use std::collections::HashMap;

use async_trait::async_trait;
use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, PaginatedRequestParams, ReadResourceRequestParams};
use rmcp::service::{Peer, RoleClient, RunningService};
use rmcp::transport::{StreamableHttpClientTransport, TokioChildProcess};
use serde_json::Value;
use tokio::sync::Mutex as AsyncMutex;

use crate::error::{DaemonError, DaemonResult};

/// A single tool exposed by a remote MCP server.
#[derive(Debug, Clone)]
pub struct RemoteTool {
    pub name: String,
    pub description: String,
    /// JSON Schema of the tool arguments.
    pub schema: Value,
}

/// A resource exposed by a remote MCP server.
#[derive(Debug, Clone)]
pub struct RemoteResource {
    pub uri: String,
    pub name: String,
    pub description: String,
    pub mime_type: String,
}

/// Transport-agnostic operations on one MCP server connection.
///
/// The trait isolates the rmcp types so hosts and tests can substitute fake
/// implementations.
#[async_trait]
pub trait McpConnectionOps: Send + Sync {
    async fn list_tools(&self) -> DaemonResult<Vec<RemoteTool>>;
    async fn call_tool(&self, name: &str, args: Value) -> DaemonResult<String>;
    async fn list_resources(&self) -> DaemonResult<Vec<RemoteResource>>;
    async fn read_resource_text(&self, uri: &str) -> DaemonResult<String>;

    /// Terminates the underlying session or process.
    ///
    /// Safe to call while other references exist: it waits for in-flight
    /// operations on this connection to finish before cancelling.
    async fn shutdown(&self);
}

/// Credential references are resolved by the owner, never exposed by adapters.
pub(crate) struct GuardedConnection {
    pub inner: RmcpConnection,
    pub secrets: Vec<String>,
}
impl GuardedConnection {
    fn text(&self, mut value: String) -> DaemonResult<String> {
        if value.len() > 1024 * 1024 {
            return Err(DaemonError::Mcp("Addon MCP response limit exceeded".into()));
        }
        for secret in &self.secrets {
            if !secret.is_empty() {
                value = value.replace(secret, "[redacted]");
            }
        }
        Ok(value)
    }
    fn json(&self, value: &mut Value) -> DaemonResult<()> {
        match value {
            Value::String(s) => *s = self.text(s.clone())?,
            Value::Array(a) => {
                for v in a {
                    self.json(v)?;
                }
            }
            Value::Object(o) => {
                let old = std::mem::take(o);
                for (k, mut v) in old {
                    self.json(&mut v)?;
                    o.insert(self.text(k)?, v);
                }
            }
            _ => {}
        }
        Ok(())
    }
}
fn owned_error(_: DaemonError) -> DaemonError {
    DaemonError::Mcp("Addon MCP request failed or connection closed".into())
}
#[async_trait]
impl McpConnectionOps for GuardedConnection {
    async fn list_tools(&self) -> DaemonResult<Vec<RemoteTool>> {
        let mut tools = self.inner.list_tools().await.map_err(owned_error)?;
        for tool in &mut tools {
            if self.text(tool.name.clone())? != tool.name {
                return Err(owned_error(DaemonError::Mcp(String::new())));
            }
            tool.description = self.text(tool.description.clone())?;
            self.json(&mut tool.schema)?;
        }
        Ok(tools)
    }
    async fn call_tool(&self, name: &str, args: Value) -> DaemonResult<String> {
        self.text(self.inner.call_tool(name, args).await.map_err(owned_error)?)
    }
    async fn list_resources(&self) -> DaemonResult<Vec<RemoteResource>> {
        let mut resources = self.inner.list_resources().await.map_err(owned_error)?;
        for r in &mut resources {
            r.uri = self.text(r.uri.clone())?;
            r.name = self.text(r.name.clone())?;
            r.description = self.text(r.description.clone())?;
            r.mime_type = self.text(r.mime_type.clone())?;
        }
        Ok(resources)
    }
    async fn read_resource_text(&self, uri: &str) -> DaemonResult<String> {
        self.text(self.inner.read_resource_text(uri).await.map_err(owned_error)?)
    }
    async fn shutdown(&self) {
        self.inner.shutdown().await;
    }
}

/// An active rmcp client session.
///
/// The service lives inside an [`AsyncMutex`] so [`McpConnectionOps::shutdown`]
/// can take ownership for `cancel()` even while callers only hold shared
/// references to the connection.
pub struct RmcpConnection {
    service: AsyncMutex<Option<RunningService<RoleClient, ()>>>,
    /// Managed addon processes retain the child handle until tree cleanup, so
    /// an exited/reused PID cannot be mistaken for an owned process.
    child: AsyncMutex<Option<OwnedChild>>,
}

pub(crate) struct OwnedChild(pub(crate) Option<tokio::process::Child>);
impl OwnedChild {
    pub(crate) async fn shutdown(mut self) {
        if let Some(mut child) = self.0.take() {
            crate::execution::jobs::terminate(&mut child).await;
        }
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let Some(mut child) = self.0.take() else {
            return;
        };
        #[cfg(unix)]
        if let Some(pid) = child.id() {
            // This child still has an unreaped owned handle and led its group.
            unsafe {
                libc::killpg(pid as i32, libc::SIGKILL);
            }
        }
        #[cfg(windows)]
        if let Some(pid) = child.id() {
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .creation_flags(0x08000000)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
        let _ = child.start_kill();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = child.wait().await;
            });
        }
    }
}

impl RmcpConnection {
    pub(crate) async fn connect_stdio_owned(
        command: &[String],
        env: &HashMap<String, String>,
        cwd: &std::path::Path,
    ) -> DaemonResult<Self> {
        let (program, args) = command
            .split_first()
            .ok_or_else(|| DaemonError::Mcp("Missing addon MCP executable".into()))?;
        let mut command = tokio::process::Command::new(program);
        command
            .args(args)
            .env_clear()
            .envs(env)
            .current_dir(cwd)
            .kill_on_drop(true)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        #[cfg(unix)]
        command.process_group(0);
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let mut child = OwnedChild(Some(
            command
                .spawn()
                .map_err(|_| DaemonError::Mcp("Addon MCP process could not start".into()))?,
        ));
        let process = child.0.as_mut().expect("owned child");
        let stdout = process
            .stdout
            .take()
            .ok_or_else(|| DaemonError::Mcp("Addon MCP stdout unavailable".into()))?;
        let stdin = process
            .stdin
            .take()
            .ok_or_else(|| DaemonError::Mcp("Addon MCP stdin unavailable".into()))?;
        let service =
            ().serve((stdout, stdin))
                .await
                .map_err(|_| DaemonError::Mcp("Addon MCP initialization failed".into()))?;
        Ok(Self {
            service: AsyncMutex::new(Some(service)),
            child: AsyncMutex::new(Some(child)),
        })
    }

    pub(crate) async fn connect_http_owned(
        url: &str,
        authorization: Option<&str>,
    ) -> DaemonResult<Self> {
        use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .connect_timeout(std::time::Duration::from_secs(5))
            .build()
            .map_err(|_| DaemonError::Mcp("Addon MCP HTTP client unavailable".into()))?;
        let mut config = StreamableHttpClientTransportConfig::with_uri(url);
        config.auth_header = authorization.map(str::to_owned);
        config.reinit_on_expired_session = false;
        config.max_sse_event_size = 1024 * 1024;
        let service = ()
            .serve(StreamableHttpClientTransport::with_client(client, config))
            .await
            .map_err(|_| DaemonError::Mcp("Addon MCP HTTP initialization failed".into()))?;
        Ok(Self {
            service: AsyncMutex::new(Some(service)),
            child: AsyncMutex::new(None),
        })
    }
    /// Clones the live service peer, failing after shutdown.
    async fn peer(&self) -> DaemonResult<Peer<RoleClient>> {
        let guard = self.service.lock().await;
        guard
            .as_ref()
            .map(|service| service.peer().clone())
            .ok_or_else(|| DaemonError::Mcp("connection already closed".to_string()))
    }

    /// Connects to an MCP server spawned as a child process (stdio).
    pub async fn connect_stdio(
        command: &[String],
        env: &HashMap<String, String>,
    ) -> DaemonResult<Self> {
        let (program, args) = command.split_first().ok_or_else(|| {
            DaemonError::Mcp("stdio transport requires a non-empty command".to_string())
        })?;
        let mut cmd = tokio::process::Command::new(program);
        cmd.args(args).envs(env);
        let transport = TokioChildProcess::new(cmd)
            .map_err(|e| DaemonError::Mcp(format!("failed to spawn MCP server: {e}")))?;
        Self::start_stdio(transport).await
    }

    /// Connects to an MCP server over Streamable HTTP.
    pub async fn connect_http(url: &str) -> DaemonResult<Self> {
        let transport = StreamableHttpClientTransport::from_uri(url.to_string());
        Self::start_http(transport).await
    }

    async fn start_stdio(transport: TokioChildProcess) -> DaemonResult<Self> {
        let service = ()
            .serve(transport)
            .await
            .map_err(|err| DaemonError::Mcp(format!("MCP initialization failed: {err}")))?;
        Ok(Self {
            service: AsyncMutex::new(Some(service)),
            child: AsyncMutex::new(None),
        })
    }

    async fn start_http(
        transport: StreamableHttpClientTransport<reqwest::Client>,
    ) -> DaemonResult<Self> {
        let service = ()
            .serve(transport)
            .await
            .map_err(|err| DaemonError::Mcp(format!("MCP initialization failed: {err}")))?;
        Ok(Self {
            service: AsyncMutex::new(Some(service)),
            child: AsyncMutex::new(None),
        })
    }
}

#[async_trait]
impl McpConnectionOps for RmcpConnection {
    async fn list_tools(&self) -> DaemonResult<Vec<RemoteTool>> {
        let result = self
            .peer()
            .await?
            .list_tools(Some(PaginatedRequestParams::default()))
            .await
            .map_err(mcp_error)?;
        Ok(result
            .tools
            .into_iter()
            .map(|tool| RemoteTool {
                name: tool.name.to_string(),
                description: tool.description.as_deref().unwrap_or("").to_string(),
                schema: serde_json::to_value(tool.input_schema.as_ref())
                    .unwrap_or(serde_json::json!({"type": "object"})),
            })
            .collect())
    }

    async fn call_tool(&self, name: &str, args: Value) -> DaemonResult<String> {
        let arguments = match args {
            Value::Object(map) => map,
            _ => serde_json::Map::new(),
        };
        let params = CallToolRequestParams::new(name.to_owned()).with_arguments(arguments);
        let result = self.peer().await?.call_tool(params).await.map_err(mcp_error)?;
        if result.is_error.unwrap_or(false) {
            return Err(DaemonError::Mcp(format!(
                "tool {name} returned an error: {}",
                content_to_string(&result.content)
            )));
        }
        Ok(content_to_string(&result.content))
    }

    async fn list_resources(&self) -> DaemonResult<Vec<RemoteResource>> {
        let result = self
            .peer()
            .await?
            .list_resources(Some(PaginatedRequestParams::default()))
            .await
            .map_err(mcp_error)?;
        Ok(result
            .resources
            .into_iter()
            .map(|resource| RemoteResource {
                uri: resource.uri,
                name: resource.name,
                description: resource.description.unwrap_or_default(),
                mime_type: resource.mime_type.unwrap_or_default(),
            })
            .collect())
    }

    async fn read_resource_text(&self, uri: &str) -> DaemonResult<String> {
        let result = self
            .peer()
            .await?
            .read_resource(ReadResourceRequestParams::new(uri.to_string()))
            .await
            .map_err(mcp_error)?;
        let mut text = String::new();
        for content in result.contents {
            match content {
                rmcp::model::ResourceContents::TextResourceContents {
                    text: piece,
                    ..
                } => {
                    text.push_str(&piece);
                }
                rmcp::model::ResourceContents::BlobResourceContents {
                    blob,
                    ..
                } => {
                    text.push_str(&format!("\n[binary resource: {} bytes]", blob.len()));
                }
                _ => {}
            }
        }
        Ok(text)
    }

    async fn shutdown(&self) {
        // Taking the lock waits for any in-flight operation on this
        // connection; `take()` guarantees exactly-once cancellation even if
        // shutdown itself is called twice.
        let child = self.child.lock().await.take();
        if let Some(child) = child {
            child.shutdown().await;
        }
        if let Some(service) = self.service.lock().await.take() {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(3), service.cancel()).await;
        }
    }
}

/// Flattens response content into plain text (text parts joined by newlines).
fn content_to_string(content: &[rmcp::model::ContentBlock]) -> String {
    content
        .iter()
        .filter_map(|item| match item {
            rmcp::model::ContentBlock::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn mcp_error(err: rmcp::ServiceError) -> DaemonError {
    DaemonError::Mcp(err.to_string())
}
