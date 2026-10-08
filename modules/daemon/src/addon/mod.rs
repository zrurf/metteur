//! Addon host: package lifecycle, tool registration and prompt fragments.

pub mod manifest;
pub mod package;
pub mod runtime;
pub mod signature;
pub mod signer;

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use metteur_shared::Value;

use crate::error::{DaemonError, DaemonResult};
use crate::registry::Tool;

pub(crate) mod hooks;
mod host;
pub(crate) mod functions;
mod lsp;
mod nodes;
pub(crate) mod services;
pub use host::{AddonHost, AddonInfoData};

#[cfg(test)]
mod lifecycle_tests;
#[cfg(all(test, unix))]
mod lsp_tests;
#[cfg(test)]
mod mcp_tests;

/// A tool owns verified immutable code and the grants for that exact package.
struct AddonTool {
    package: Arc<package::Package>,
    function: String,
    registered_name: String,
    description: String,
    parameters_schema: serde_json::Value,
    permissions: HashSet<runtime::Permission>,
    timeout: u64,
}
impl AddonTool {
    fn new(
        package: Arc<package::Package>,
        entry: &manifest::ToolEntry,
        permissions: HashSet<runtime::Permission>,
        timeout: u64,
    ) -> Self {
        Self {
            registered_name: format!("{}{}", pascal(&package.identity.id), entry.name),
            function: entry.function.clone(),
            description: entry.description.clone(),
            parameters_schema: toml_table_to_json(&entry.parameters),
            package,
            permissions,
            timeout,
        }
    }
}
#[async_trait::async_trait]
impl Tool for AddonTool {
    fn name(&self) -> &str {
        &self.registered_name
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn parameters(&self) -> serde_json::Value {
        self.parameters_schema.clone()
    }
    async fn call(
        &self,
        args: &[Value],
        ctx: &mut crate::execution::context::ExecutionContext,
    ) -> DaemonResult<Value> {
        let input = serde_json::json!({"args": args_to_object(args)}).to_string();
        self.invoke_json(input, ctx).await
    }
}
impl AddonTool {
    async fn invoke_json(
        &self,
        input: String,
        ctx: &mut crate::execution::context::ExecutionContext,
    ) -> DaemonResult<Value> {
        ctx.attach_file_journal();
        let mut execution = ctx.child_nested();
        execution.permission_mode = ctx.permission_mode;
        let runtime = tokio::runtime::Handle::current();
        let permissions = self.permissions.clone();
        let package = self.package.clone();
        let function = self.function.clone();
        let timeout = self.timeout;
        let (output, reads, full_reads, mutations) = tokio::task::spawn_blocking(move || {
            let context = Arc::new(runtime::InvocationContext {
                runtime,
                permissions,
                execution: parking_lot::Mutex::new(execution),
                http: reqwest::blocking::Client::builder()
                    .timeout(std::time::Duration::from_secs(10))
                    .build()
                    .map_err(|_| {
                        DaemonError::Addon("Cannot initialize addon HTTP client".into())
                    })?,
            });
            let output = runtime::invoke(
                &package.wasm,
                &package.manifest,
                &function,
                &input,
                context.clone(),
                timeout,
            );
            let mut execution = context.execution.lock();
            Ok::<_, DaemonError>((
                output,
                std::mem::take(&mut execution.read_paths),
                std::mem::take(&mut execution.read_paths_full),
                std::mem::take(&mut execution.mutated_paths),
            ))
        })
        .await
        .map_err(|_| DaemonError::Addon("Addon worker failed".into()))??;
        ctx.read_paths.extend(reads);
        ctx.read_paths_full.extend(full_reads);
        ctx.mutated_paths.extend(mutations);
        let output = output?;
        let parsed: serde_json::Value = serde_json::from_str(&output)
            .map_err(|_| DaemonError::Addon("Addon returned invalid JSON".into()))?;
        Ok(Value::Json(parsed))
    }
}

fn args_to_object(args: &[Value]) -> serde_json::Map<String, serde_json::Value> {
    let mut map = serde_json::Map::new();
    for value in args {
        if let Value::Json(serde_json::Value::Object(entries)) = value {
            for (key, item) in entries {
                map.insert(key.clone(), item.clone());
            }
        } else if let Value::String(text) = value {
            // Positional string arguments become {"input": text}.
            map.insert("input".to_string(), serde_json::Value::String(text.clone()));
        }
    }
    map
}

fn pascal(input: &str) -> String {
    input
        .split(['.', '-', '_', ' '])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        })
        .collect()
}

fn toml_table_to_json(table: &toml::Table) -> serde_json::Value {
    // Convert through TOML text to reuse the lossless serializer.
    toml::to_string(table)
        .ok()
        .and_then(|text| text.parse::<toml::Table>().ok())
        .map(|converted| toml_value_to_json(&toml::Value::Table(converted)))
        .unwrap_or(serde_json::json!({"type": "object"}))
}

fn toml_value_to_json(value: &toml::Value) -> serde_json::Value {
    match value {
        toml::Value::Boolean(flag) => serde_json::Value::Bool(*flag),
        toml::Value::Integer(number) => serde_json::Value::Number((*number).into()),
        toml::Value::Float(number) => serde_json::Number::from_f64(*number)
            .map_or(serde_json::Value::Null, serde_json::Value::Number),
        toml::Value::String(text) => serde_json::Value::String(text.clone()),
        toml::Value::Datetime(text) => serde_json::Value::String(text.to_string()),
        toml::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(toml_value_to_json).collect())
        }
        toml::Value::Table(map) => {
            let mut object = serde_json::Map::new();
            for (key, item) in map {
                object.insert(key.clone(), toml_value_to_json(item));
            }
            serde_json::Value::Object(object)
        }
    }
}

/// Extract into a fresh owned staging directory with bounded, portable paths.
fn unzip_into(archive_path: &Path, target: &Path) -> DaemonResult<()> {
    use std::io::{Read, Write};
    let file = std::fs::File::open(archive_path)?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|_| DaemonError::Addon("Invalid zip package".into()))?;
    if archive.len() > 1024 {
        return Err(DaemonError::Addon("Too many zip entries".into()));
    }
    std::fs::create_dir(target)?;
    let mut names = HashSet::new();
    let mut total = 0usize;
    for index in 0..archive.len() {
        let mut entry =
            archive.by_index(index).map_err(|_| DaemonError::Addon("Invalid zip entry".into()))?;
        let name = entry.name().trim_end_matches('/').to_string();
        manifest::validate_package_path(&name)?;
        if !names.insert(name.clone())
            || entry.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000)
        {
            return Err(DaemonError::Addon("Duplicate entry or link in zip package".into()));
        }
        let dest = target.join(&name);
        if entry.is_dir() {
            std::fs::create_dir_all(dest)?;
            continue;
        }
        let mut bytes = Vec::new();
        (&mut entry).take((64 * 1024 * 1024 - total + 1) as u64).read_to_end(&mut bytes)?;
        total += bytes.len();
        if total > 64 * 1024 * 1024 {
            return Err(DaemonError::Addon("Package exceeds 64 MiB".into()));
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::OpenOptions::new().write(true).create_new(true).open(dest)?.write_all(&bytes)?;
    }
    Ok(())
}
