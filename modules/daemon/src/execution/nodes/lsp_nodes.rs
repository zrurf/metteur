//! Deterministic validation from language-server diagnostics.
//!
//! [`LspCheck`] turns "the edited code must still compile" into a rule the
//! blueprint engine can enforce without asking an LLM. It is a sibling of the
//! `Validator` node: the interpreter treats a failing `Passed` output as a
//! validation failure, so the existing rollback/retry and circuit-breaker
//! machinery applies unchanged. Warnings are tolerated by default because most
//! language servers report style-level noise that must not fail a build.

use std::collections::HashMap;

use async_trait::async_trait;
use metteur_shared::{Node, PinId, Value};

use crate::error::{DaemonError, DaemonResult};
use crate::execution::context::ExecutionContext;
use crate::registry::NodeExecutor;
use crate::workspace::fs::WorkspaceFs;

use super::{bool_output, int_output, is_empty, json_to_value, string_output};

/// Severity values used by the LSP specification (1 = Error, 2 = Warning).
const SEVERITY_ERROR: u64 = 1;
const SEVERITY_WARNING: u64 = 2;

/// Runs the configured language server over one or more files and reports
/// whether they are error-free.
pub struct LspCheckExecutor;

#[async_trait]
impl NodeExecutor for LspCheckExecutor {
    fn kind(&self) -> &str {
        "LspCheck"
    }

    async fn execute(
        &self,
        node: &Node,
        inputs: &HashMap<PinId, Value>,
        ctx: &mut ExecutionContext,
    ) -> DaemonResult<HashMap<PinId, Value>> {
        let paths = collect_paths(node, inputs)?;
        let timeout_ms = super::input_or_data(node, inputs, "TimeoutMs")
            .ok()
            .and_then(|value| value.as_int())
            .filter(|value| *value > 0)
            .unwrap_or(5000) as u64;
        let allow_warnings =
            node.data.get("allow_warnings").and_then(|value| value.as_bool()).unwrap_or(true);

        // No language server configured is not a failure: the rule simply has
        // nothing to observe, and failing here would break every workspace
        // that has not set up LSP.
        let Some(manager) = ctx.lsp_manager() else {
            return Ok(empty_report(node, allow_warnings));
        };

        let mut errors = 0u64;
        let mut warnings = 0u64;
        let mut diagnostics = Vec::new();
        for path in &paths {
            let outcome = diagnose(&manager, ctx, path, timeout_ms).await;
            match outcome {
                Ok(entries) => {
                    for entry in entries {
                        let severity =
                            entry.get("severity").and_then(|value| value.as_u64()).unwrap_or(0);
                        match severity {
                            SEVERITY_ERROR => errors += 1,
                            SEVERITY_WARNING => warnings += 1,
                            _ => {}
                        }
                        diagnostics.push(serde_json::json!({
                            "path": path,
                            "severity": severity,
                            "message": entry
                                .get("message")
                                .and_then(|value| value.as_str())
                                .unwrap_or(""),
                            "range": entry.get("range").cloned().unwrap_or(serde_json::Value::Null),
                        }));
                    }
                }
                // An unreadable or unroutable file is reported as one error:
                // silently passing would hide a real problem from the plan.
                Err(err) => {
                    errors += 1;
                    diagnostics.push(serde_json::json!({
                        "path": path,
                        "severity": SEVERITY_ERROR,
                        "message": err.to_string(),
                        "range": serde_json::Value::Null,
                    }));
                }
            }
        }

        let passed = errors == 0 && (allow_warnings || warnings == 0);
        ctx.audit(
            "lsp.check",
            serde_json::json!({
                "run_id": ctx.run_id.to_string(),
                "node_id": node.id.to_string(),
                "paths": paths,
                "errors": errors,
                "warnings": warnings,
                "passed": passed,
            }),
        );

        // A hand-written blueprint may omit the counter pins; those are
        // informational, so a missing pin is skipped rather than fatal.
        let mut outputs = bool_output(node, "Passed", passed)?;
        if let Ok(errors_pin) = int_output(node, "Errors", errors as i64) {
            outputs.extend(errors_pin);
        }
        if let Ok(warnings_pin) = int_output(node, "Warnings", warnings as i64) {
            outputs.extend(warnings_pin);
        }
        let summary =
            string_output(node, "Diagnostics", diag_text(&diagnostics, errors, warnings))?;
        outputs.extend(summary);
        // A structured list lets a blueprint branch on individual problems
        // instead of re-parsing the summary text.
        if let Some(pin) = node.pins.iter().find(|p| p.name == "Problems") {
            outputs.insert(pin.id, diagnostics_value(&diagnostics));
        }
        Ok(outputs)
    }
}

/// Builds the pass-through report used when LSP is unavailable.
fn empty_report(node: &Node, allow_warnings: bool) -> HashMap<PinId, Value> {
    let mut outputs = HashMap::new();
    if let Ok(passed) = bool_output(node, "Passed", true) {
        outputs.extend(passed);
    }
    if let Ok(errors) = int_output(node, "Errors", 0) {
        outputs.extend(errors);
    }
    if let Ok(warnings) = int_output(node, "Warnings", 0) {
        outputs.extend(warnings);
    }
    if let Ok(text) = string_output(
        node,
        "Diagnostics",
        if allow_warnings {
            "[lsp: not configured; check skipped]"
        } else {
            "[lsp: not configured; warnings would fail this check]"
        },
    ) {
        outputs.extend(text);
    }
    if let Some(pin) = node.pins.iter().find(|p| p.name == "Problems") {
        outputs.insert(pin.id, Value::List(Vec::new()));
    }
    outputs
}

/// Returns the workspace-relative paths to check.
///
/// `Path` takes precedence, then a `paths` list in `node.data` for checking a
/// whole set in one node.
fn collect_paths(node: &Node, inputs: &HashMap<PinId, Value>) -> DaemonResult<Vec<String>> {
    // The `Path` pin is the normal source; `data.path` covers nodes authored
    // without the pin (a minimal canvas or a hand-written blueprint).
    if let Ok(value) = super::input_or_data(node, inputs, "Path")
        && !is_empty(&value)
    {
        return Ok(vec![value_to_path(&value)?]);
    }
    if let Some(path) = node.data.get("path").and_then(|value| value.as_str())
        && !path.is_empty()
    {
        return Ok(vec![path.to_string()]);
    }
    // Note: every path returned here is resolved through `WorkspaceFs` by the
    // caller, so a blueprint cannot read outside its workspace.
    if let Some(paths) = node.data.get("paths").and_then(|value| value.as_array()) {
        let mut collected = Vec::new();
        for entry in paths {
            if let Some(text) = entry.as_str().filter(|text| !text.is_empty()) {
                collected.push(text.to_string());
            }
        }
        if !collected.is_empty() {
            return Ok(collected);
        }
    }
    Err(DaemonError::Execution("LspCheck requires a Path input or a `paths` list".to_string()))
}

fn value_to_path(value: &Value) -> DaemonResult<String> {
    match value {
        Value::String(path) => Ok(path.clone()),
        Value::Json(json) => json
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| DaemonError::Execution("LspCheck Path must be a string".to_string())),
        _ => Err(DaemonError::Execution("LspCheck Path must be a string".to_string())),
    }
}

/// Returns the raw diagnostics of one file (same pipeline as the tool).
async fn diagnose(
    manager: &std::sync::Arc<crate::integration::lsp::LspManager>,
    ctx: &ExecutionContext,
    path: &str,
    timeout_ms: u64,
) -> DaemonResult<Vec<serde_json::Value>> {
    let fs = WorkspaceFs::new(ctx.workspace_root.clone());
    let absolute = fs.resolve_existing(path)?;
    let extension =
        absolute.extension().and_then(|ext| ext.to_str()).unwrap_or_default().to_string();
    let Some(client) = manager.client_for_extension(&extension).await? else {
        return Err(DaemonError::NotFound(format!(
            "no language server configured for .{extension}"
        )));
    };
    let uri = manager.to_file_uri(&absolute);
    let content = fs.read(path)?;
    let since = client.current_epoch();
    let language_id = manager.language_for_extension(&extension).unwrap_or(extension.clone());
    client.sync_document(&uri, &String::from_utf8_lossy(&content), &language_id).await?;
    client.wait_document(&uri, since, timeout_ms).await?;
    let mut items = Vec::new();
    for entry in client.diagnostics_snapshot().await {
        if entry.uri == uri {
            items.extend(entry.diagnostics.iter().cloned());
        }
    }
    Ok(items)
}

/// Renders a compact human/model-readable diagnostics report.
fn diag_text(diagnostics: &[serde_json::Value], errors: u64, warnings: u64) -> String {
    if diagnostics.is_empty() {
        return "no diagnostics".to_string();
    }
    let mut out = format!("{errors} error(s), {warnings} warning(s)\n");
    for entry in diagnostics.iter().take(30) {
        let path = entry.get("path").and_then(|value| value.as_str()).unwrap_or("");
        let line = entry
            .pointer("/range/start/line")
            .and_then(|value| value.as_u64())
            .map(|line| line + 1)
            .unwrap_or(0);
        let message = entry.get("message").and_then(|value| value.as_str()).unwrap_or("");
        out.push_str(&format!("{path}:{line}: {message}\n"));
    }
    if diagnostics.len() > 30 {
        out.push_str(&format!("... [{} more]\n", diagnostics.len() - 30));
    }
    out
}

/// Converts a JSON diagnostics array into a shared [`Value`] list.
///
/// Exposed as a `Diagnostics` data pin so a blueprint can branch on the
/// structured list rather than re-parsing the text summary.
pub(crate) fn diagnostics_value(items: &[serde_json::Value]) -> Value {
    Value::List(items.iter().map(json_to_value).collect())
}
