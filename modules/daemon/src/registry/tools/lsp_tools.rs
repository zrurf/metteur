//! LSP-backed built-in tools: diagnostics, hover and go-to-definition.

use std::sync::Arc;

use async_trait::async_trait;
use metteur_shared::{ToolResultLifetime, Value};

use crate::error::{DaemonError, DaemonResult};
use crate::execution::context::ExecutionContext;
use crate::integration::lsp::{LspManager, client::LspClient};
use crate::registry::Tool;
use crate::workspace::fs::WorkspaceFs;

/// One resolved document pushed to its language server.
struct DocumentTarget {
    client: Arc<LspClient>,
    uri: String,
}

fn json_field<'a>(value: &'a Value, name: &str) -> Option<&'a serde_json::Value> {
    match value {
        Value::Json(serde_json::Value::Object(map)) => map.get(name),
        _ => None,
    }
}

fn string_input(args: &[Value], name: &str) -> DaemonResult<String> {
    args.iter()
        .find_map(|value| json_field(value, name).and_then(|item| item.as_str().map(String::from)))
        .ok_or_else(|| DaemonError::Execution(format!("missing input {name}")))
}

fn u32_input(args: &[Value], name: &str) -> DaemonResult<u32> {
    args.iter()
        .find_map(|value| json_field(value, name).and_then(|item| item.as_u64()))
        .map(|value| value as u32)
        .ok_or_else(|| DaemonError::Execution(format!("missing input {name}")))
}

fn extension_of(path: &std::path::Path) -> String {
    path.extension().and_then(|ext| ext.to_str()).unwrap_or_default().to_string()
}

/// Resolves the document and pushes its current content to the language
/// server (didOpen on first touch, didChange afterwards).
async fn open_document(
    manager: &LspManager,
    fs: &WorkspaceFs,
    path: &str,
) -> DaemonResult<DocumentTarget> {
    let absolute = fs.resolve_existing(path)?;
    let extension = extension_of(&absolute);
    let Some(client) = manager.client_for_extension(&extension).await? else {
        return Err(DaemonError::NotFound(format!(
            "no language server configured for .{extension}"
        )));
    };
    let uri = manager.to_file_uri(&absolute);
    let content = fs.read(path)?;
    let language_id = manager.language_for_extension(&extension).unwrap_or(extension.clone());
    client.sync_document(&uri, &String::from_utf8_lossy(&content), &language_id).await?;
    Ok(DocumentTarget {
        client,
        uri,
    })
}

fn require_manager(ctx: &ExecutionContext) -> DaemonResult<Arc<LspManager>> {
    ctx.lsp_manager().ok_or_else(|| {
        DaemonError::Lsp("LSP integration is not enabled for this workspace".to_string())
    })
}

/// Normalized diagnostic entry.
fn normalize_diagnostic(item: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "range": item.get("range").cloned().unwrap_or(serde_json::json!({})),
        "severity": item.get("severity").cloned().unwrap_or(serde_json::Value::Null),
        "message": item.get("message").and_then(|v| v.as_str()).unwrap_or(""),
        "source": item.get("source").cloned().unwrap_or(serde_json::Value::Null),
    })
}

/// Extracts plain text from an LSP `Hover.contents` value.
fn hover_text(hover: &serde_json::Value) -> String {
    match hover.get("contents") {
        Some(serde_json::Value::String(text)) => text.clone(),
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                serde_json::Value::String(text) => text.clone(),
                other => other.get("value").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            })
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n"),
        Some(contents) => {
            contents.get("value").and_then(|value| value.as_str()).unwrap_or_default().to_string()
        }
        None => String::new(),
    }
}

/// Converts a file URI back to a workspace-relative display path.
fn relative_path(manager: &LspManager, uri: &str) -> String {
    metteur_shared::Uri::parse(uri)
        .ok()
        .and_then(|parsed| parsed.to_path().ok())
        .and_then(|absolute| {
            absolute
                .strip_prefix(manager.workspace_root())
                .ok()
                .map(|relative| relative.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/"))
        })
        .unwrap_or_else(|| uri.to_string())
}

/// Normalizes a definition response into `{path, line, character}` entries.
fn normalize_definitions(raw: &serde_json::Value, manager: &LspManager) -> Vec<serde_json::Value> {
    fn push_location(
        out: &mut Vec<serde_json::Value>,
        manager: &LspManager,
        location: &serde_json::Value,
    ) {
        // `Location` carries `uri`; `LocationLink` carries `targetUri`.
        let uri = location
            .get("uri")
            .or_else(|| location.get("targetUri"))
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let start = location
            .get("range")
            .or_else(|| location.get("targetSelectionRange"))
            .and_then(|range| range.get("start"))
            .cloned()
            .unwrap_or(serde_json::json!({}));
        if uri.is_empty() {
            return;
        }
        out.push(serde_json::json!({
            "path": relative_path(manager, uri),
            "line": start.get("line").and_then(|v| v.as_u64()).unwrap_or(0),
            "character": start.get("character").and_then(|v| v.as_u64()).unwrap_or(0),
        }));
    }
    let mut locations = Vec::new();
    match raw {
        serde_json::Value::Null => {}
        serde_json::Value::Array(items) => {
            for item in items {
                push_location(&mut locations, manager, item);
            }
        }
        location @ serde_json::Value::Object(_) => push_location(&mut locations, manager, location),
        _ => {}
    }
    locations
}

/// Returns diagnostics for one file (`CheckDiagnostics`).
pub struct CheckDiagnostics;

#[async_trait]
impl Tool for CheckDiagnostics {
    fn name(&self) -> &str {
        "CheckDiagnostics"
    }

    fn description(&self) -> &str {
        "Runs the configured language server on a file and returns its \
         current diagnostics. Files without a configured language server \
         yield an empty list."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "timeout_ms": {"type": "integer", "minimum": 100}
            },
            "required": ["path"]
        })
    }

    fn read_only(&self) -> bool {
        true
    }

    fn lifetime(&self) -> ToolResultLifetime {
        ToolResultLifetime::Persistent
    }

    async fn call(&self, args: &[Value], ctx: &mut ExecutionContext) -> DaemonResult<Value> {
        let empty = || Value::Json(serde_json::json!({"diagnostics": []}));
        // No LSP manager (integration disabled) behaves like "no diagnostics".
        let Some(manager) = ctx.lsp_manager() else {
            return Ok(empty());
        };
        let path = string_input(args, "path")?;
        let timeout_ms = args
            .iter()
            .find_map(|value| json_field(value, "timeout_ms").and_then(|item| item.as_u64()))
            .unwrap_or(5000);

        let fs = WorkspaceFs::new(ctx.workspace_root.clone());
        let absolute = fs.resolve_existing(&path)?;
        let extension = extension_of(&absolute);
        let Some(client) = manager.client_for_extension(&extension).await? else {
            return Ok(empty());
        };
        let uri = manager.to_file_uri(&absolute);
        let content = fs.read(&path)?;

        // An explicit request always syncs: the caller asked for the current
        // state, so merging it away would defeat the point.
        let since = client.current_epoch();
        let decision = manager.should_sync(&uri, true).await;
        if decision == crate::integration::lsp::debounce::SyncDecision::SyncNow {
            let language_id =
                manager.language_for_extension(&extension).unwrap_or(extension.clone());
            client.sync_document(&uri, &String::from_utf8_lossy(&content), &language_id).await?;
        }
        client.wait_document(&uri, since, timeout_ms).await?;
        let mut items = Vec::new();
        for entry in client.diagnostics_snapshot().await {
            if entry.uri == uri {
                items.extend(entry.diagnostics.iter().map(normalize_diagnostic));
            }
        }
        Ok(Value::Json(serde_json::json!({ "diagnostics": items })))
    }
}

/// Returns hover information at a position (`GetHover`).
pub struct GetHover;

#[async_trait]
impl Tool for GetHover {
    fn name(&self) -> &str {
        "GetHover"
    }

    fn description(&self) -> &str {
        "Returns hover documentation for the symbol at a 0-based line and \
         character position in a workspace file."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "line": {"type": "integer"},
                "character": {"type": "integer"}
            },
            "required": ["path", "line", "character"]
        })
    }

    fn read_only(&self) -> bool {
        true
    }

    fn lifetime(&self) -> ToolResultLifetime {
        ToolResultLifetime::Persistent
    }

    async fn call(&self, args: &[Value], ctx: &mut ExecutionContext) -> DaemonResult<Value> {
        let manager = require_manager(ctx)?;
        let path = string_input(args, "path")?;
        let line = u32_input(args, "line")?;
        let character = u32_input(args, "character")?;
        let fs = WorkspaceFs::new(ctx.workspace_root.clone());
        let target = open_document(&manager, &fs, &path).await?;
        let hover = target.client.hover(&target.uri, line, character).await?;
        Ok(Value::Json(serde_json::json!({
            "content": hover_text(&hover),
            "range": hover.get("range").cloned().unwrap_or(serde_json::Value::Null),
        })))
    }
}

/// Returns definition locations for a symbol (`FindDefinition`).
pub struct FindDefinition;

#[async_trait]
impl Tool for FindDefinition {
    fn name(&self) -> &str {
        "FindDefinition"
    }

    fn description(&self) -> &str {
        "Finds where the symbol at a 0-based line and character position is \
         defined. Returns a list of {path, line, character} entries with \
         workspace-relative paths."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "line": {"type": "integer"},
                "character": {"type": "integer"}
            },
            "required": ["path", "line", "character"]
        })
    }

    fn read_only(&self) -> bool {
        true
    }

    fn lifetime(&self) -> ToolResultLifetime {
        ToolResultLifetime::Persistent
    }

    async fn call(&self, args: &[Value], ctx: &mut ExecutionContext) -> DaemonResult<Value> {
        let manager = require_manager(ctx)?;
        let path = string_input(args, "path")?;
        let line = u32_input(args, "line")?;
        let character = u32_input(args, "character")?;
        let fs = WorkspaceFs::new(ctx.workspace_root.clone());
        let target = open_document(&manager, &fs, &path).await?;
        let result = target.client.definition(&target.uri, line, character).await?;
        Ok(Value::Json(
            serde_json::json!({ "definitions": normalize_definitions(&result, &manager) }),
        ))
    }
}
