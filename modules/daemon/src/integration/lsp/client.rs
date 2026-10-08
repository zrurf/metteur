//! A minimal LSP client speaking JSON-RPC over framed stdio streams.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Mutex as AsyncMutex, oneshot};

use super::rpc::{Frame, encode_frame, parse_frame};
use crate::error::{DaemonError, DaemonResult};

type PendingMap = HashMap<i64, oneshot::Sender<Result<Value, String>>>;
type Writer = Arc<AsyncMutex<Box<dyn tokio::io::AsyncWrite + Send + Unpin>>>;

/// One cached `publishDiagnostics` payload for a document.
#[derive(Debug, Clone)]
pub struct DiagnosticsEntry {
    pub uri: String,
    pub diagnostics: Vec<Value>,
}

/// A client connection to one language server.
pub struct LspClient {
    writer: Writer,
    next_id: AtomicI64,
    pending: Arc<AsyncMutex<PendingMap>>,
    diagnostics: Arc<AsyncMutex<HashMap<String, DiagnosticsEntry>>>,
    /// Bumped whenever any diagnostics notification arrives.
    diagnostics_epoch: Arc<AtomicU64>,
    /// Set by [`LspClient::shutdown`] to stop the reader task.
    closed: Arc<AtomicU64>,
    documents: Arc<AsyncMutex<HashMap<String, i64>>>,
    timeout_ms: u64,
    guarded: bool,
    reader: tokio::task::AbortHandle,
}

impl LspClient {
    /// Wraps an existing byte stream pair (child stdio or test duplexes).
    pub fn over_streams(
        read: Box<dyn tokio::io::AsyncRead + Send + Unpin>,
        write: Box<dyn tokio::io::AsyncWrite + Send + Unpin>,
    ) -> Arc<Self> {
        Self::streams(read, write, 30_000, vec![], false)
    }
    pub(crate) fn guarded(
        read: Box<dyn tokio::io::AsyncRead + Send + Unpin>,
        write: Box<dyn tokio::io::AsyncWrite + Send + Unpin>,
        timeout_ms: u64,
        secrets: Vec<String>,
    ) -> Arc<Self> {
        Self::streams(read, write, timeout_ms.clamp(1, 30_000), secrets, true)
    }
    fn streams(
        read: Box<dyn tokio::io::AsyncRead + Send + Unpin>,
        write: Box<dyn tokio::io::AsyncWrite + Send + Unpin>,
        timeout_ms: u64,
        secrets: Vec<String>,
        guarded: bool,
    ) -> Arc<Self> {
        let writer: Writer = Arc::new(AsyncMutex::new(write));
        let pending: Arc<AsyncMutex<PendingMap>> = Arc::new(AsyncMutex::new(HashMap::new()));
        let diagnostics = Arc::new(AsyncMutex::new(HashMap::new()));
        let epoch = Arc::new(AtomicU64::new(0));
        let closed = Arc::new(AtomicU64::new(0));
        let documents = Arc::new(AsyncMutex::new(HashMap::<String, i64>::new()));

        let reader = {
            let mut read = read;
            let task_pending = pending.clone();
            let task_diagnostics = diagnostics.clone();
            let task_epoch = epoch.clone();
            let task_writer = writer.clone();
            let task_closed = closed.clone();
            let task_documents = documents.clone();
            tokio::spawn(async move {
                let mut buffer: Vec<u8> = Vec::new();
                loop {
                    if task_closed.load(Ordering::SeqCst) != 0 || buffer.len() > 1024 * 1024 {
                        break;
                    }
                    // Drain every complete frame currently buffered.
                    loop {
                        match parse_frame(&buffer) {
                            Frame::Complete {
                                payload,
                                consumed,
                            } => {
                                let mut payload = payload.to_vec();
                                buffer.drain(..consumed);
                                if guarded {
                                    let Ok(mut message) = serde_json::from_slice::<Value>(&payload)
                                    else {
                                        continue;
                                    };
                                    redact(&mut message, &secrets);
                                    if message.get("method").and_then(Value::as_str)
                                        == Some("textDocument/publishDiagnostics")
                                    {
                                        let uri = message
                                            .pointer("/params/uri")
                                            .and_then(Value::as_str)
                                            .unwrap_or("");
                                        let docs = task_documents.lock().await;
                                        let Some(version) = docs.get(uri) else {
                                            continue;
                                        };
                                        if message
                                            .pointer("/params/version")
                                            .and_then(Value::as_i64)
                                            .is_some_and(|v| v != *version)
                                        {
                                            continue;
                                        }
                                    }
                                    payload = serde_json::to_vec(&message).unwrap_or_default();
                                }
                                handle_message(
                                    &task_pending,
                                    &task_diagnostics,
                                    &task_epoch,
                                    &task_writer,
                                    &payload,
                                )
                                .await;
                                if task_diagnostics.lock().await.len() > 1024 {
                                    task_closed.store(1, Ordering::SeqCst);
                                    break;
                                }
                            }
                            // Resynchronize past a corrupt header block so
                            // the stream cannot wedge or grow without bound.
                            Frame::Malformed {
                                consumed,
                            } => {
                                buffer.drain(..consumed);
                            }
                            Frame::Incomplete => break,
                        }
                    }
                    if buffer.len() > 1024 * 1024 || task_closed.load(Ordering::SeqCst) != 0 {
                        break;
                    }
                    let mut chunk = [0u8; 8192];
                    match read.read(&mut chunk).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => buffer.extend_from_slice(&chunk[..n]),
                    }
                }
                task_closed.store(1, Ordering::SeqCst);
                task_pending.lock().await.clear();
                task_diagnostics.lock().await.clear();
            })
            .abort_handle()
        };

        Arc::new(Self {
            writer,
            next_id: AtomicI64::new(1),
            pending,
            diagnostics,
            diagnostics_epoch: epoch,
            closed,
            documents,
            timeout_ms,
            guarded,
            reader,
        })
    }

    async fn send_raw(&self, message: &Value) -> DaemonResult<()> {
        if self.is_closed() {
            return Err(DaemonError::Lsp("language server connection closed".into()));
        }
        let payload =
            serde_json::to_vec(message).map_err(|err| DaemonError::Lsp(err.to_string()))?;
        let frame = encode_frame(&payload);
        let result =
            tokio::time::timeout(std::time::Duration::from_millis(self.timeout_ms), async {
                let mut writer = self.writer.lock().await;
                writer.write_all(&frame).await?;
                writer.flush().await
            })
            .await;
        match result {
            Ok(Ok(())) => Ok(()),
            _ => {
                self.shutdown().await;
                Err(DaemonError::Lsp("language server write failed or timed out".into()))
            }
        }
    }

    /// Sends a request and awaits its response result.
    pub async fn request(&self, method: &str, params: Value) -> DaemonResult<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            if pending.len() >= 256 {
                return Err(DaemonError::Lsp(
                    "language server pending request limit exceeded".into(),
                ));
            }
            pending.insert(id, tx);
        }
        if let Err(err) = self.send_raw(&message).await {
            self.pending.lock().await.remove(&id);
            return Err(err);
        }
        let response =
            tokio::time::timeout(std::time::Duration::from_millis(self.timeout_ms), rx).await;
        self.pending.lock().await.remove(&id);
        match response {
            Ok(Ok(Ok(result))) => Ok(result),
            Ok(Ok(Err(message))) => Err(DaemonError::Lsp(if self.guarded {
                "Addon LSP request failed".into()
            } else {
                message
            })),
            _ => {
                self.shutdown().await;
                Err(DaemonError::Lsp("language server response failed or timed out".into()))
            }
        }
    }

    /// Sends a notification (no response expected).
    pub async fn notify(&self, method: &str, params: Value) -> DaemonResult<()> {
        self.send_raw(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
        .await
    }

    /// Performs the initialize/initialized handshake.
    pub async fn initialize(&self, workspace_root: &Path) -> DaemonResult<()> {
        let root_uri = metteur_shared::Uri::from_path(workspace_root).to_string();
        let params = json!({
            "processId": std::process::id(),
            "capabilities": {},
            "rootUri": root_uri,
        });
        self.request("initialize", params).await?;
        self.notify("initialized", json!({})).await
    }

    /// Pushes the full text of a document (didOpen or didChange, full sync).
    pub async fn sync_document(
        &self,
        uri: &str,
        text: &str,
        language_id: &str,
    ) -> DaemonResult<()> {
        let mut documents = self.documents.lock().await;
        if !documents.contains_key(uri) && documents.len() >= 1024 {
            return Err(DaemonError::Lsp("language server document limit exceeded".into()));
        }
        let version = documents.entry(uri.to_owned()).or_insert(0);
        *version += 1;
        let version = *version;
        drop(documents);
        let known = version > 1;
        if self.guarded {
            self.diagnostics.lock().await.remove(uri);
        }
        if known {
            self.notify(
                "textDocument/didChange",
                json!({
                    "textDocument": {"uri": uri, "version": version},
                    "contentChanges": [{"text": text}],
                }),
            )
            .await
        } else {
            self.notify(
                "textDocument/didOpen",
                json!({
                    "textDocument": {
                        "uri": uri,
                        "languageId": language_id,
                        "version": version,
                        "text": text,
                    },
                }),
            )
            .await
        }
    }

    /// Requests hover information; returns the raw result value.
    pub async fn hover(&self, uri: &str, line: u32, character: u32) -> DaemonResult<Value> {
        self.request("textDocument/hover", position_params(uri, line, character)).await
    }

    /// Requests go-to-definition locations; returns the raw result value.
    pub async fn definition(&self, uri: &str, line: u32, character: u32) -> DaemonResult<Value> {
        self.request("textDocument/definition", position_params(uri, line, character)).await
    }

    /// Waits until diagnostics advance past `since_epoch` or timeout.
    pub async fn wait_diagnostics(&self, since_epoch: u64, timeout_ms: u64) {
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms.max(1));
        while self.diagnostics_epoch.load(Ordering::SeqCst) <= since_epoch {
            if std::time::Instant::now() >= deadline {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        // Small quiet window to coalesce bursts of publishes.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    /// Owned servers must publish current document diagnostics or explicitly fail.
    pub async fn wait_document(
        &self,
        uri: &str,
        since_epoch: u64,
        timeout_ms: u64,
    ) -> DaemonResult<()> {
        if !self.guarded {
            self.wait_diagnostics(since_epoch, timeout_ms).await;
            return Ok(());
        }
        let deadline = tokio::time::Instant::now()
            + std::time::Duration::from_millis(timeout_ms.max(1).min(self.timeout_ms));
        loop {
            if self.is_closed() {
                return Err(DaemonError::Lsp("Addon LSP connection failed".into()));
            }
            if self.diagnostics.lock().await.contains_key(uri) {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                self.shutdown().await;
                return Err(DaemonError::Lsp("Addon LSP diagnostics timed out".into()));
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst) != 0
    }

    /// Returns the current epoch counter for diagnostics updates.
    pub fn current_epoch(&self) -> u64 {
        self.diagnostics_epoch.load(Ordering::SeqCst)
    }

    /// Returns the cached diagnostic entries keyed by document URI.
    pub async fn diagnostics_snapshot(&self) -> Vec<DiagnosticsEntry> {
        self.diagnostics.lock().await.values().cloned().collect()
    }

    /// Marks the client closed; the reader task exits on its next wake-up.
    pub async fn shutdown(&self) {
        self.closed.store(1, Ordering::SeqCst);
        self.reader.abort();
        self.pending.lock().await.clear();
        self.diagnostics.lock().await.clear();
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

fn redact(value: &mut Value, secrets: &[String]) {
    match value {
        Value::String(text) => {
            for secret in secrets {
                if !secret.is_empty() {
                    *text = text.replace(secret, "[redacted]");
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                redact(value, secrets);
            }
        }
        Value::Object(values) => {
            let old = std::mem::take(values);
            for (key, mut value) in old {
                let mut key = Value::String(key);
                redact(&mut key, secrets);
                redact(&mut value, secrets);
                values.insert(key.as_str().unwrap_or_default().into(), value);
            }
        }
        _ => {}
    }
}

fn position_params(uri: &str, line: u32, character: u32) -> Value {
    json!({
        "textDocument": {"uri": uri},
        "position": {"line": line, "character": character},
    })
}

/// Routes one decoded message to pending requests, the diagnostics cache or
/// a synthesized reply for server-to-client requests.
async fn handle_message(
    pending: &Arc<AsyncMutex<PendingMap>>,
    diagnostics: &Arc<AsyncMutex<HashMap<String, DiagnosticsEntry>>>,
    epoch: &Arc<AtomicU64>,
    writer: &Writer,
    payload: &[u8],
) {
    let Ok(message) = serde_json::from_slice::<Value>(payload) else {
        return;
    };
    let id = message.get("id").and_then(|value| value.as_i64());
    let method = message.get("method").and_then(|value| value.as_str()).map(String::from);

    if let Some(id) = id {
        let is_response = message.get("result").is_some() || message.get("error").is_some();
        if is_response {
            let result = match message.get("error") {
                Some(error) => Err(error
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown error")
                    .to_string()),
                None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
            };
            if let Some(sender) = pending.lock().await.remove(&id) {
                let _ = sender.send(result);
            }
            return;
        }
        // Reply with null to keep servers that probe capabilities happy.
        let reply = json!({"jsonrpc": "2.0", "id": id, "result": Value::Null});
        let frame = encode_frame(&serde_json::to_vec(&reply).unwrap_or_default());
        let mut guard = writer.lock().await;
        let _ = guard.write_all(&frame).await;
        let _ = guard.flush().await;
        return;
    }

    if method.as_deref() == Some("textDocument/publishDiagnostics")
        && let Some(params) = message.get("params")
    {
        let uri =
            params.get("uri").and_then(|value| value.as_str()).unwrap_or_default().to_string();
        let items = params
            .get("diagnostics")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        diagnostics.lock().await.insert(
            uri.clone(),
            DiagnosticsEntry {
                uri,
                diagnostics: items,
            },
        );
        epoch.fetch_add(1, Ordering::SeqCst);
    }
}
